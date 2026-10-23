/// Wire format spoken to the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Protocol {
    /// OpenAI Chat Completions (`/chat/completions`, `/embeddings`).
    OpenAi,
    /// Anthropic Messages (`/v1/messages`).
    Anthropic,
    /// OpenAI Responses (`/responses`), on the provider's OpenAI base URL.
    Responses,
}

/// A known API host and the protocols it speaks.
///
/// Use a built-in constant, or build your own: every field is public.
/// For a server that is not listed, skip the provider and set
/// [`base_url`](crate::ClientBuilder::base_url) + [`protocol`](crate::ClientBuilder::protocol).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Provider {
    /// Short lowercase name, e.g. `"groq"`.
    pub name: &'static str,
    /// Base URL of the OpenAI-compatible API (`/chat/completions` is appended).
    pub openai_url: Option<&'static str>,
    /// Base URL of the Anthropic-compatible API (`/v1/messages` is appended).
    pub anthropic_url: Option<&'static str>,
    /// Protocol used when none is chosen.
    pub protocol: Protocol,
    /// Env var read when no API key is given. Empty: no key needed.
    pub api_key_env: &'static str,
    /// Serves OpenAI-compatible `/embeddings`.
    pub embeddings: bool,
    /// Anthropic endpoint wants `Authorization: Bearer` instead of `x-api-key`.
    pub anthropic_bearer: bool,
    /// JSON field for the output token limit on the OpenAI protocol.
    pub max_tokens_field: &'static str,
    /// Serves the OpenAI Responses API (`/responses`).
    pub responses: bool,
    /// OpenAI protocol: send reasoning back as `reasoning_content` on assistant turns.
    /// DeepSeek, Moonshot and Z.ai need it in tool loops; strict servers reject the field.
    pub send_reasoning: bool,
}

const fn openai(name: &'static str, url: &'static str, env: &'static str, embeddings: bool) -> Provider {
    Provider {
        name,
        openai_url: Some(url),
        anthropic_url: None,
        protocol: Protocol::OpenAi,
        api_key_env: env,
        embeddings,
        anthropic_bearer: false,
        max_tokens_field: "max_tokens",
        responses: false,
        send_reasoning: false,
    }
}

impl Provider {
    const fn anthropic(mut self, url: &'static str, bearer: bool) -> Self {
        self.anthropic_url = Some(url);
        self.anthropic_bearer = bearer;
        self
    }

    const fn responses(mut self) -> Self {
        self.responses = true;
        self
    }

    const fn send_reasoning(mut self) -> Self {
        self.send_reasoning = true;
        self
    }

    // Verified against provider docs on 2026-10-22.
    /// OpenAI.
    pub const OPENAI: Provider = Provider {
        max_tokens_field: "max_completion_tokens",
        ..openai("openai", "https://api.openai.com/v1", "OPENAI_API_KEY", true).responses()
    };
    /// Anthropic. Defaults to the native Messages protocol.
    pub const ANTHROPIC: Provider = Provider {
        protocol: Protocol::Anthropic,
        ..openai("anthropic", "https://api.anthropic.com/v1", "ANTHROPIC_API_KEY", false)
            .anthropic("https://api.anthropic.com", false)
    };
    /// Groq.
    pub const GROQ: Provider = openai("groq", "https://api.groq.com/openai/v1", "GROQ_API_KEY", false).responses();
    /// Cerebras.
    pub const CEREBRAS: Provider = openai("cerebras", "https://api.cerebras.ai/v1", "CEREBRAS_API_KEY", false);
    /// OpenRouter.
    pub const OPENROUTER: Provider = openai("openrouter", "https://openrouter.ai/api/v1", "OPENROUTER_API_KEY", true)
        .anthropic("https://openrouter.ai/api", true)
        .responses();
    /// MiniMax (international).
    pub const MINIMAX: Provider = openai("minimax", "https://api.minimax.io/v1", "MINIMAX_API_KEY", false)
        .anthropic("https://api.minimax.io/anthropic", true);
    /// Atlas Cloud.
    pub const ATLAS_CLOUD: Provider = openai("atlascloud", "https://api.atlascloud.ai/v1", "ATLASCLOUD_API_KEY", false)
        .anthropic("https://api.atlascloud.ai", false);
    /// GMI Cloud.
    pub const GMI_CLOUD: Provider = openai("gmicloud", "https://api.gmi-serving.com/v1", "GMI_API_KEY", false)
        .anthropic("https://api.gmi-serving.com", true);
    /// DeepSeek.
    pub const DEEPSEEK: Provider = openai("deepseek", "https://api.deepseek.com", "DEEPSEEK_API_KEY", false)
        .anthropic("https://api.deepseek.com/anthropic", false)
        .send_reasoning();
    /// Moonshot / Kimi (international).
    pub const MOONSHOT: Provider = openai("moonshot", "https://api.moonshot.ai/v1", "MOONSHOT_API_KEY", false)
        .anthropic("https://api.moonshot.ai/anthropic", true)
        .send_reasoning();
    /// Z.ai (GLM).
    pub const ZAI: Provider = openai("zai", "https://api.z.ai/api/paas/v4", "ZAI_API_KEY", false)
        .anthropic("https://api.z.ai/api/anthropic", true)
        .send_reasoning();
    /// Together AI.
    pub const TOGETHER: Provider = openai("together", "https://api.together.ai/v1", "TOGETHER_API_KEY", true);
    /// Fireworks AI.
    pub const FIREWORKS: Provider =
        openai("fireworks", "https://api.fireworks.ai/inference/v1", "FIREWORKS_API_KEY", true)
            .anthropic("https://api.fireworks.ai/inference", true);
    /// Mistral.
    pub const MISTRAL: Provider = openai("mistral", "https://api.mistral.ai/v1", "MISTRAL_API_KEY", true);
    /// xAI (Grok).
    pub const XAI: Provider = openai("xai", "https://api.x.ai/v1", "XAI_API_KEY", false).responses();
    /// Google Gemini (OpenAI-compatible endpoint).
    pub const GEMINI: Provider =
        openai("gemini", "https://generativelanguage.googleapis.com/v1beta/openai", "GEMINI_API_KEY", true);
    /// DeepInfra.
    pub const DEEPINFRA: Provider =
        openai("deepinfra", "https://api.deepinfra.com/v1/openai", "DEEPINFRA_API_KEY", true)
            .anthropic("https://api.deepinfra.com/anthropic", false);
    /// Hugging Face Inference Providers router.
    pub const HUGGINGFACE: Provider = openai("huggingface", "https://router.huggingface.co/v1", "HF_TOKEN", false)
        .anthropic("https://router.huggingface.co", true);
    /// Nebius Token Factory.
    pub const NEBIUS: Provider = openai("nebius", "https://api.tokenfactory.nebius.com/v1", "NEBIUS_API_KEY", true);
    /// SambaNova.
    pub const SAMBANOVA: Provider = openai("sambanova", "https://api.sambanova.ai/v1", "SAMBANOVA_API_KEY", false)
        .anthropic("https://api.sambanova.ai", false);
    /// Ollama on localhost. No API key.
    pub const OLLAMA: Provider =
        openai("ollama", "http://localhost:11434/v1", "", true).anthropic("http://localhost:11434", false).responses();

    /// Every built-in provider.
    pub const ALL: &'static [Provider] = &[
        Self::OPENAI,
        Self::ANTHROPIC,
        Self::GROQ,
        Self::CEREBRAS,
        Self::OPENROUTER,
        Self::MINIMAX,
        Self::ATLAS_CLOUD,
        Self::GMI_CLOUD,
        Self::DEEPSEEK,
        Self::MOONSHOT,
        Self::ZAI,
        Self::TOGETHER,
        Self::FIREWORKS,
        Self::MISTRAL,
        Self::XAI,
        Self::GEMINI,
        Self::DEEPINFRA,
        Self::HUGGINGFACE,
        Self::NEBIUS,
        Self::SAMBANOVA,
        Self::OLLAMA,
    ];

    /// Look up a built-in provider by [`name`](Provider::name).
    pub fn by_name(name: &str) -> Option<Provider> {
        Self::ALL.iter().find(|p| p.name.eq_ignore_ascii_case(name)).copied()
    }

    /// Base URL for `protocol`, or `None` if the provider does not speak it.
    pub fn url(&self, protocol: Protocol) -> Option<&'static str> {
        match protocol {
            Protocol::OpenAi => self.openai_url,
            Protocol::Anthropic => self.anthropic_url,
            Protocol::Responses => self.openai_url.filter(|_| self.responses),
        }
    }

    /// Whether the provider speaks `protocol`.
    pub fn supports(&self, protocol: Protocol) -> bool {
        self.url(protocol).is_some()
    }
}
