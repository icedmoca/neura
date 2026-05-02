use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ActiveProvider {
    Local,
    Claude,
    OpenAI,
    Copilot,
    Antigravity,
    Gemini,
    Cursor,
    OpenRouter,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct ProviderAvailability {
    pub(super) openai: bool,
    pub(super) claude: bool,
    pub(super) copilot: bool,
    pub(super) antigravity: bool,
    pub(super) gemini: bool,
    pub(super) cursor: bool,
    pub(super) openrouter: bool,
    pub(super) copilot_premium_zero: bool,
}

impl ProviderAvailability {
    pub(super) fn is_configured(self, provider: ActiveProvider) -> bool {
        match provider {
            ActiveProvider::Local => true,
            ActiveProvider::Claude => self.claude,
            ActiveProvider::OpenAI => self.openai,
            ActiveProvider::Copilot => self.copilot,
            ActiveProvider::Antigravity => self.antigravity,
            ActiveProvider::Gemini => self.gemini,
            ActiveProvider::Cursor => self.cursor,
            ActiveProvider::OpenRouter => self.openrouter,
        }
    }
}

impl MultiProvider {
    pub(super) fn auto_default_provider(availability: ProviderAvailability) -> ActiveProvider {
        if availability.copilot_premium_zero && availability.copilot {
            ActiveProvider::Copilot
        } else if availability.openai {
            ActiveProvider::OpenAI
        } else if availability.claude {
            ActiveProvider::Claude
        } else if availability.copilot {
            ActiveProvider::Copilot
        } else if availability.antigravity {
            ActiveProvider::Antigravity
        } else if availability.gemini {
            ActiveProvider::Gemini
        } else if availability.cursor {
            ActiveProvider::Cursor
        } else if availability.openrouter {
            ActiveProvider::OpenRouter
        } else {
            ActiveProvider::Claude
        }
    }

    pub(super) fn parse_provider_hint(value: &str) -> Option<ActiveProvider> {
        match value.trim().to_ascii_lowercase().as_str() {
            "local" | "oss" | "gpt-oss" => Some(ActiveProvider::Local),
            "claude" | "anthropic" => Some(ActiveProvider::Claude),
            "openai" => Some(ActiveProvider::OpenAI),
            "copilot" => Some(ActiveProvider::Copilot),
            "antigravity" => Some(ActiveProvider::Antigravity),
            "gemini" => Some(ActiveProvider::Gemini),
            "cursor" => Some(ActiveProvider::Cursor),
            "openrouter" => Some(ActiveProvider::OpenRouter),
            _ => None,
        }
    }

    pub(super) fn forced_provider_from_env() -> Option<ActiveProvider> {
        let force = std::env::var("KCODE_FORCE_PROVIDER")
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        if !force {
            return None;
        }

        std::env::var("KCODE_ACTIVE_PROVIDER")
            .ok()
            .and_then(|value| Self::parse_provider_hint(&value))
    }

    pub(super) fn provider_label(provider: ActiveProvider) -> &'static str {
        match provider {
            ActiveProvider::Local => "Local OSS",
            ActiveProvider::Claude => "Anthropic",
            ActiveProvider::OpenAI => "OpenAI",
            ActiveProvider::Copilot => "GitHub Copilot",
            ActiveProvider::Antigravity => "Antigravity",
            ActiveProvider::Gemini => "Gemini",
            ActiveProvider::Cursor => "Cursor",
            ActiveProvider::OpenRouter => "OpenRouter",
        }
    }

    pub(super) fn provider_key(provider: ActiveProvider) -> &'static str {
        match provider {
            ActiveProvider::Local => "local",
            ActiveProvider::Claude => "claude",
            ActiveProvider::OpenAI => "openai",
            ActiveProvider::Copilot => "copilot",
            ActiveProvider::Antigravity => "antigravity",
            ActiveProvider::Gemini => "gemini",
            ActiveProvider::Cursor => "cursor",
            ActiveProvider::OpenRouter => "openrouter",
        }
    }

    pub(super) fn set_active_provider(&self, provider: ActiveProvider) {
        *self
            .active
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = provider;
    }
}
