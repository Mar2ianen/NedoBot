use crate::{ChatGptProfile, ProfileId, ProviderError, Result};
use async_trait::async_trait;
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

#[async_trait]
// `async_trait` сохраняет object safety для использования через `Arc<dyn CredentialStore>`;
// сгенерированные futures уже помечены `must_use`, поэтому это предупреждение избыточно.
#[allow(clippy::double_must_use)]
pub trait CredentialStore: Send + Sync {
    async fn host_id_or_create(&self) -> Result<String>;
    async fn profile(&self, id: &ProfileId) -> Result<Option<ChatGptProfile>>;
    async fn profiles(&self) -> Result<Vec<ChatGptProfile>>;
    async fn save_profile(&self, profile: &ChatGptProfile) -> Result<()>;
    async fn delete_profile(&self, id: &ProfileId) -> Result<()>;
    async fn lock_profile(&self, id: &ProfileId) -> Result<Box<dyn Send>>;
}

#[derive(Default)]
pub struct MemoryCredentialStore {
    state: Mutex<MemoryState>,
    profile_locks: Mutex<HashMap<ProfileId, Arc<Mutex<()>>>>,
}

#[derive(Default)]
struct MemoryState {
    host_id: Option<String>,
    profiles: HashMap<ProfileId, ChatGptProfile>,
}

#[async_trait]
impl CredentialStore for MemoryCredentialStore {
    async fn host_id_or_create(&self) -> Result<String> {
        let mut state = self.state.lock().await;
        Ok(state
            .host_id
            .get_or_insert_with(ChatGptProfile::new_host_id)
            .clone())
    }

    async fn profile(&self, id: &ProfileId) -> Result<Option<ChatGptProfile>> {
        Ok(self.state.lock().await.profiles.get(id).cloned())
    }

    async fn profiles(&self) -> Result<Vec<ChatGptProfile>> {
        Ok(self.state.lock().await.profiles.values().cloned().collect())
    }

    async fn save_profile(&self, profile: &ChatGptProfile) -> Result<()> {
        self.state
            .lock()
            .await
            .profiles
            .insert(profile.id.clone(), profile.clone());
        Ok(())
    }

    async fn delete_profile(&self, id: &ProfileId) -> Result<()> {
        self.state.lock().await.profiles.remove(id);
        Ok(())
    }

    async fn lock_profile(&self, id: &ProfileId) -> Result<Box<dyn Send>> {
        let lock = {
            let mut locks = self.profile_locks.lock().await;
            Arc::clone(
                locks
                    .entry(id.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        Ok(Box::new(lock.lock_owned().await))
    }
}

/// JSON-хранилище учётных данных с закрытыми правами файлов и межпроцессными блокировками refresh.
/// Если система предоставляет хранилище ключей, приложение должно предпочесть его этому варианту.
#[derive(Clone)]
pub struct FileCredentialStore {
    root: PathBuf,
    io_lock: Arc<Mutex<()>>,
    profile_locks: Arc<Mutex<HashMap<ProfileId, Arc<Mutex<()>>>>>,
}

impl FileCredentialStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            io_lock: Arc::new(Mutex::new(())),
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }

    fn locks_dir(&self) -> PathBuf {
        self.root.join("locks")
    }

    fn profile_path(&self, id: &ProfileId) -> PathBuf {
        self.profiles_dir()
            .join(format!("{}.json", stable_file_key(id.as_str())))
    }

    async fn prepare_private_dir(path: &Path) -> Result<()> {
        tokio::fs::create_dir_all(path).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
        }
        Ok(())
    }

    async fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
        let parent = path.parent().ok_or(ProviderError::InvalidConfiguration)?;
        Self::prepare_private_dir(parent).await?;
        let tmp_path = parent.join(format!(".{}.tmp", Uuid::new_v4()));
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let mut file = options.open(&tmp_path).await?;
        let result = async {
            file.write_all(data).await?;
            file.sync_all().await?;
            drop(file);
            tokio::fs::rename(&tmp_path, path).await?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
            }
            Ok::<(), std::io::Error>(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&tmp_path).await;
        }
        result?;
        Ok(())
    }

    async fn read_profile_file(path: &Path) -> Result<Option<ChatGptProfile>> {
        match tokio::fs::read(path).await {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

#[async_trait]
impl CredentialStore for FileCredentialStore {
    async fn host_id_or_create(&self) -> Result<String> {
        let _guard = self.io_lock.lock().await;
        Self::prepare_private_dir(&self.locks_dir()).await?;
        let lock_path = self.locks_dir().join("host-id.lock");
        let host_lock = tokio::task::spawn_blocking(move || {
            let mut options = OpenOptions::new();
            options.create(true).truncate(false).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(lock_path)?;
            file.lock_exclusive()?;
            Ok::<_, std::io::Error>(file)
        })
        .await
        .map_err(std::io::Error::other)??;

        let path = self.root.join("host-id");
        let result = match tokio::fs::read_to_string(&path).await {
            Ok(value) => Ok(value.trim().to_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let host_id = ChatGptProfile::new_host_id();
                Self::atomic_write(&path, host_id.as_bytes()).await?;
                Ok(host_id)
            }
            Err(error) => Err(error.into()),
        };
        drop(host_lock);
        result
    }

    async fn profile(&self, id: &ProfileId) -> Result<Option<ChatGptProfile>> {
        let _guard = self.io_lock.lock().await;
        Self::read_profile_file(&self.profile_path(id)).await
    }

    async fn profiles(&self) -> Result<Vec<ChatGptProfile>> {
        let _guard = self.io_lock.lock().await;
        let mut entries = match tokio::fs::read_dir(self.profiles_dir()).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut profiles = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().is_some_and(|ext| ext == "json")
                && let Some(profile) = Self::read_profile_file(&entry.path()).await?
            {
                profiles.push(profile);
            }
        }
        profiles.sort_by(|left, right| left.email.cmp(&right.email));
        Ok(profiles)
    }

    async fn save_profile(&self, profile: &ChatGptProfile) -> Result<()> {
        let _guard = self.io_lock.lock().await;
        let data = serde_json::to_vec(profile)?;
        Self::atomic_write(&self.profile_path(&profile.id), &data).await
    }

    async fn delete_profile(&self, id: &ProfileId) -> Result<()> {
        let _guard = self.io_lock.lock().await;
        match tokio::fs::remove_file(self.profile_path(id)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn lock_profile(&self, id: &ProfileId) -> Result<Box<dyn Send>> {
        let process_lock = {
            let mut locks = self.profile_locks.lock().await;
            Arc::clone(
                locks
                    .entry(id.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let process_guard: OwnedMutexGuard<()> = process_lock.lock_owned().await;
        let lock_path = self
            .locks_dir()
            .join(format!("{}.lock", stable_file_key(id.as_str())));
        Self::prepare_private_dir(&self.locks_dir()).await?;
        let lock_file = tokio::task::spawn_blocking(move || {
            let mut options = OpenOptions::new();
            options.create(true).truncate(false).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(lock_path)?;
            file.lock_exclusive()?;
            Ok::<_, std::io::Error>(file)
        })
        .await
        .map_err(std::io::Error::other)??;
        Ok(Box::new(FileProfileLock {
            _process_guard: process_guard,
            _file: lock_file,
        }))
    }
}

struct FileProfileLock {
    _process_guard: OwnedMutexGuard<()>,
    _file: std::fs::File,
}

impl Drop for FileProfileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self._file);
    }
}

fn stable_file_key(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenSet;
    use std::time::Duration;

    fn sample_profile() -> ChatGptProfile {
        ChatGptProfile {
            id: ProfileId::from_client_id("oaiapp_example".to_owned()),
            client_id: "oaiapp_example".to_owned(),
            host_id: "urn:uuid:8d8cc35c-314f-4534-8e5b-f818efaf28c1".to_owned(),
            issuer: "https://auth.openai.com".to_owned(),
            subject: "subject-1".to_owned(),
            email: Some("person@example.com".to_owned()),
            display_name: None,
            tokens: Some(TokenSet {
                access_token: "access-secret".to_owned(),
                refresh_token: "refresh-secret".to_owned(),
                id_token: Some("id-secret".to_owned()),
                token_type: "Bearer".to_owned(),
                scopes: vec![
                    "chatgpt.tokens.use.direct".to_owned(),
                    "resource.invoke".to_owned(),
                ],
                expires_at_unix: 1_800_000_000,
                earliest_refresh_at_unix: None,
            }),
        }
    }

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("provider-access-test-{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn file_store_writes_private_profiles_and_redacts_debug() {
        let root = temp_path();
        let store = FileCredentialStore::new(&root);
        let profile = sample_profile();
        store.save_profile(&profile).await.unwrap();
        let loaded = store.profile(&profile.id).await.unwrap().unwrap();
        let debug = format!("{loaded:?}");

        assert!(loaded.plan_usage_enabled());
        assert!(!debug.contains("access-secret"));
        assert!(!debug.contains("refresh-secret"));
        assert!(!debug.contains("id-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = tokio::fs::metadata(store.profile_path(&profile.id))
                .await
                .unwrap()
                .permissions();
            assert_eq!(permissions.mode() & 0o777, 0o600);
        }
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn file_store_creates_one_host_id_across_store_instances() {
        let root = temp_path();
        let first = FileCredentialStore::new(&root);
        let second = FileCredentialStore::new(&root);
        let (left, right) = tokio::join!(first.host_id_or_create(), second.host_id_or_create());

        let left = left.unwrap();
        assert_eq!(left, right.unwrap());
        assert!(left.starts_with("urn:uuid:"));
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn profile_refresh_lock_serializes_store_instances() {
        let root = temp_path();
        let first = FileCredentialStore::new(&root);
        let second = FileCredentialStore::new(&root);
        let profile = sample_profile();
        let held = first.lock_profile(&profile.id).await.unwrap();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let id = profile.id.clone();
        let second_task = tokio::spawn(async move {
            let _guard = second.lock_profile(&id).await.unwrap();
            let _ = sender.send(());
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(40), receiver)
                .await
                .is_err()
        );
        drop(held);
        tokio::time::timeout(Duration::from_secs(2), second_task)
            .await
            .unwrap()
            .unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
