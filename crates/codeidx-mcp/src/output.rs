use codeidx_protocol::{ToolError, ToolPayload};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Compact,
    Json,
    Markdown,
}

impl OutputFormat {
    pub(crate) fn parse(args: &Value) -> Result<Self, ToolError> {
        match args.get("format") {
            None => Ok(Self::Compact),
            Some(v) => match v.as_str() {
                Some("compact") => Ok(Self::Compact),
                Some("json") => Ok(Self::Json),
                Some("md") => Ok(Self::Markdown),
                _ => Err(ToolError::new(
                    "invalid format: expected compact, json, or md",
                )),
            },
        }
    }
    pub(crate) fn render<T: serde::Serialize>(
        self,
        report: &T,
        compact: impl FnOnce() -> String,
    ) -> ToolPayload {
        match self {
            Self::Json => {
                ToolPayload::Json(serde_json::to_value(report).expect("report DTO serializes"))
            }
            Self::Compact | Self::Markdown => self.text(compact()),
        }
    }
    pub(crate) fn text(self, text: String) -> ToolPayload {
        if self != Self::Markdown {
            return ToolPayload::Text(text);
        }
        let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
        let fence = "`".repeat(3.max(longest + 1));
        let separator = if text.ends_with('\n') { "" } else { "\n" };
        ToolPayload::Text(format!("{fence}text\n{text}{separator}{fence}\n"))
    }
}
