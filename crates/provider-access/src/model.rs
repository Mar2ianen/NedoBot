use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProvider {
    OpenAiChatGpt,
    OpenRouter,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pricing {
    /// Цена миллиона входных токенов в USD, если провайдер её публикует.
    pub input_usd_per_million: Option<f64>,
    /// Цена миллиона выходных токенов в USD, если провайдер её публикует.
    pub output_usd_per_million: Option<f64>,
}

impl Pricing {
    pub fn is_free(&self) -> bool {
        self.input_usd_per_million.is_some_and(|value| value == 0.0)
            && self
                .output_usd_per_million
                .is_some_and(|value| value == 0.0)
    }

    pub fn total(&self) -> Option<f64> {
        Some(self.input_usd_per_million? + self.output_usd_per_million?)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub provider: ModelProvider,
    pub context_length: Option<u64>,
    pub supports_text: bool,
    pub supports_images: bool,
    pub supports_tools: bool,
    pub pricing: Pricing,
    /// Динамический маршрут провайдера, например `openrouter/free`.
    pub is_router: bool,
}

impl ModelDescriptor {
    pub fn is_free(&self) -> bool {
        self.pricing.is_free()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ModelSort {
    #[default]
    FreeFirst,
    LowestPrice,
    LargestContext,
    Name,
}

#[derive(Clone, Debug, Default)]
pub struct ModelQuery {
    pub requires_images: bool,
    pub requires_tools: bool,
    pub minimum_context: Option<u64>,
    pub free_only: bool,
    pub preferred_provider: Option<ModelProvider>,
    pub preferred_ids: Vec<String>,
    pub sort: ModelSort,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectedModel {
    pub model: ModelDescriptor,
    pub score: usize,
}

pub struct ModelSelector;

impl ModelSelector {
    pub fn list<'a>(models: &'a [ModelDescriptor], query: &ModelQuery) -> Vec<&'a ModelDescriptor> {
        let mut matching = models
            .iter()
            .filter(|model| model.supports_text)
            .filter(|model| !query.requires_images || model.supports_images)
            .filter(|model| !query.requires_tools || model.supports_tools)
            .filter(|model| {
                query.minimum_context.is_none_or(|minimum| {
                    model
                        .context_length
                        .is_some_and(|context| context >= minimum)
                })
            })
            .filter(|model| !query.free_only || model.is_free())
            .collect::<Vec<_>>();

        matching.sort_by(|left, right| compare_models(left, right, query));
        matching
    }

    pub fn select(models: &[ModelDescriptor], query: &ModelQuery) -> Option<SelectedModel> {
        let model = Self::list(models, query).into_iter().next()?.clone();
        let score = query
            .preferred_ids
            .iter()
            .position(|preferred| preferred == &model.id)
            .map_or(0, |index| query.preferred_ids.len() - index + 1);
        Some(SelectedModel { model, score })
    }
}

fn compare_models(
    left: &ModelDescriptor,
    right: &ModelDescriptor,
    query: &ModelQuery,
) -> std::cmp::Ordering {
    let preferred_left = preferred_id_rank(left, query);
    let preferred_right = preferred_id_rank(right, query);
    preferred_right
        .cmp(&preferred_left)
        .then_with(|| {
            let provider_left = usize::from(query.preferred_provider != Some(left.provider));
            let provider_right = usize::from(query.preferred_provider != Some(right.provider));
            provider_left.cmp(&provider_right)
        })
        .then_with(|| match query.sort {
            ModelSort::FreeFirst => right.is_free().cmp(&left.is_free()),
            ModelSort::LowestPrice => compare_price(left, right),
            ModelSort::LargestContext => right.context_length.cmp(&left.context_length),
            ModelSort::Name => left.display_name.cmp(&right.display_name),
        })
        .then_with(|| left.id.cmp(&right.id))
}

fn preferred_id_rank(model: &ModelDescriptor, query: &ModelQuery) -> usize {
    query
        .preferred_ids
        .iter()
        .position(|preferred| preferred == &model.id)
        .map_or(0, |index| query.preferred_ids.len() - index + 1)
}

fn compare_price(left: &ModelDescriptor, right: &ModelDescriptor) -> std::cmp::Ordering {
    match (left.pricing.total(), right.pricing.total()) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, free: bool, tools: bool, images: bool, context: u64) -> ModelDescriptor {
        let price = if free { 0.0 } else { 1.0 };
        ModelDescriptor {
            id: id.to_owned(),
            display_name: id.to_owned(),
            provider: ModelProvider::OpenRouter,
            context_length: Some(context),
            supports_text: true,
            supports_images: images,
            supports_tools: tools,
            pricing: Pricing {
                input_usd_per_million: Some(price),
                output_usd_per_million: Some(price),
            },
            is_router: false,
        }
    }

    #[test]
    fn filters_for_free_tool_capable_models_with_context() {
        let models = vec![
            model("small-free", true, true, false, 16_000),
            model("large-free", true, true, true, 128_000),
            model("paid", false, true, true, 200_000),
            model("no-tools", true, false, true, 200_000),
        ];
        let query = ModelQuery {
            requires_images: true,
            requires_tools: true,
            minimum_context: Some(64_000),
            free_only: true,
            ..ModelQuery::default()
        };

        let found = ModelSelector::list(&models, &query);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "large-free");
    }

    #[test]
    fn preferred_model_wins_before_generic_sorting() {
        let models = vec![
            model("free", true, false, false, 16_000),
            model("preferred", false, false, false, 8_000),
        ];
        let query = ModelQuery {
            preferred_ids: vec!["preferred".to_owned()],
            ..ModelQuery::default()
        };

        assert_eq!(
            ModelSelector::select(&models, &query).unwrap().model.id,
            "preferred"
        );
    }
}
