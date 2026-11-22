use codeidx_project::callflow::{self, TraceDirection, TraceLocation, TraceOptions};
use codeidx_protocol::{ToolError, ToolPayload, TraceCallsParams, TraceDirectionParam};
use serde_json::Value;

use crate::handlers::parse_args;
use crate::output::OutputFormat;
use crate::{output_callflow, LSP, MANAGER};

pub(crate) async fn dispatch(args: &Value) -> Result<ToolPayload, ToolError> {
    let format = OutputFormat::parse(args)?;
    let params: TraceCallsParams = parse_args(args)?;
    let options = options(&params)?;
    let report = callflow::trace_calls(&MANAGER, Some(&LSP), &params.path, options).await?;
    Ok(format.render(&report, || output_callflow::compact(&report)))
}

fn options(params: &TraceCallsParams) -> Result<TraceOptions, ToolError> {
    if params.from.is_some() && (params.symbol.is_some() || params.file_path.is_some()) {
        return Err(ToolError::new(
            "from cannot be used with symbol or file_path",
        ));
    }
    if params.file_path.is_some() && params.symbol.is_none() {
        return Err(ToolError::new("file_path requires symbol"));
    }
    if params.symbol.as_deref().is_some_and(str::is_empty)
        || params.file_path.as_deref().is_some_and(str::is_empty)
    {
        return Err(ToolError::new("symbol and file_path must not be empty"));
    }
    if params
        .from
        .as_ref()
        .is_some_and(|from| from.line == 0 || from.file.is_empty() || from.symbol.is_empty())
    {
        return Err(ToolError::new(
            "from requires nonempty file and symbol and line >= 1",
        ));
    }
    for (name, value) in [
        ("max_roots", params.max_roots),
        ("max_nodes", params.max_nodes),
        ("budget", params.budget),
    ] {
        if value == Some(0) {
            return Err(ToolError::new(format!("{name} must be at least 1")));
        }
    }
    let defaults = TraceOptions::default();
    Ok(TraceOptions {
        symbol: params.symbol.clone(),
        file_path: params.file_path.clone(),
        from: params.from.as_ref().map(|from| TraceLocation {
            file: from.file.clone(),
            line: from.line,
            symbol: from.symbol.clone(),
        }),
        direction: match params.direction {
            None => defaults.direction,
            Some(TraceDirectionParam::Down) => TraceDirection::Down,
            Some(TraceDirectionParam::Up) => TraceDirection::Up,
            Some(TraceDirectionParam::Both) => TraceDirection::Both,
        },
        max_depth: params.max_depth.unwrap_or(defaults.max_depth),
        max_roots: params.max_roots.unwrap_or(defaults.max_roots),
        max_nodes: params.max_nodes.unwrap_or(defaults.max_nodes),
        budget: params.budget.unwrap_or(defaults.budget),
        refine_lsp: params.refine_lsp.unwrap_or(defaults.refine_lsp),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resume_and_engine_defaults_are_forwarded() {
        let args = json!({"path":"/tmp/example", "from":{"file":"foo.rs", "line":7, "symbol":"start"}, "direction":"both", "max_depth":0});
        let options = options(&parse_args(&args).unwrap()).unwrap();
        assert_eq!(options.direction, TraceDirection::Both);
        assert_eq!(options.max_depth, 0);
        assert_eq!(options.from.unwrap().line, 7);
        assert_eq!(options.budget, TraceOptions::default().budget);
        assert!(options.refine_lsp);
    }

    #[test]
    fn invalid_resume_and_budget_are_tool_errors() {
        for args in [
            json!({"path":"/tmp/example", "file_path":"foo.rs"}),
            json!({"path":"/tmp/example", "from":{"file":"foo.rs", "line":0, "symbol":"start"}}),
            json!({"path":"/tmp/example", "symbol":"start", "from":{"file":"foo.rs", "line":7, "symbol":"start"}}),
            json!({"path":"/tmp/example", "budget":0}),
        ] {
            assert!(options(&parse_args(&args).unwrap()).is_err());
        }
    }
}
