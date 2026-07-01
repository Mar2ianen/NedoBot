#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use teloxide::types::Message;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMediaKind {
    Voice,
    Audio,
    VideoNote,
}

impl VoiceMediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Voice => "voice",
            Self::Audio => "audio",
            Self::VideoNote => "video_note",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRenderMode {
    Short,
    Chapters,
    File,
}

impl TranscriptRenderMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Chapters => "chapters",
            Self::File => "file",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoiceMedia {
    pub chat_id: i64,
    pub message_id: i32,
    pub user_id: Option<i64>,
    pub kind: VoiceMediaKind,
    pub file_id: String,
    pub file_unique_id: Option<String>,
    pub duration_sec: Option<u32>,
    pub file_size: Option<u64>,
    pub mime_type: Option<String>,
}

impl VoiceMedia {
    pub fn from_message(msg: &Message) -> Option<Self> {
        if let Some(voice) = msg.voice() {
            return Some(Self {
                chat_id: msg.chat.id.0,
                message_id: msg.id.0,
                user_id: msg.from.as_ref().map(|user| user.id.0 as i64),
                kind: VoiceMediaKind::Voice,
                file_id: voice.file.id.clone(),
                file_unique_id: Some(voice.file.unique_id.clone()),
                duration_sec: Some(voice.duration.seconds()),
                file_size: Some(voice.file.size as u64),
                mime_type: voice.mime_type.as_ref().map(ToString::to_string),
            });
        }

        if let Some(audio) = msg.audio() {
            return Some(Self {
                chat_id: msg.chat.id.0,
                message_id: msg.id.0,
                user_id: msg.from.as_ref().map(|user| user.id.0 as i64),
                kind: VoiceMediaKind::Audio,
                file_id: audio.file.id.clone(),
                file_unique_id: Some(audio.file.unique_id.clone()),
                duration_sec: Some(audio.duration.seconds()),
                file_size: Some(audio.file.size as u64),
                mime_type: audio.mime_type.as_ref().map(ToString::to_string),
            });
        }

        msg.video_note().map(|video_note| Self {
            chat_id: msg.chat.id.0,
            message_id: msg.id.0,
            user_id: msg.from.as_ref().map(|user| user.id.0 as i64),
            kind: VoiceMediaKind::VideoNote,
            file_id: video_note.file.id.clone(),
            file_unique_id: Some(video_note.file.unique_id.clone()),
            duration_sec: Some(video_note.duration.seconds()),
            file_size: Some(video_note.file.size as u64),
            mime_type: None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AsrSegment {
    pub start_sec: f32,
    pub end_sec: f32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AsrTranscript {
    pub provider: String,
    pub model: String,
    pub request_id: Option<String>,
    pub text: String,
    pub segments: Vec<AsrSegment>,
    pub raw_json: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptChapter {
    pub title: String,
    pub start_sec: f32,
    pub end_sec: Option<f32>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CleanTranscript {
    pub mode: TranscriptRenderMode,
    pub text: String,
    pub chapters: Vec<TranscriptChapter>,
    pub short_summary: Option<String>,
}
