use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use ascii_dag::{Graph, RenderMode};
use cwl_core::documents::{
    CWLDocument, CommandLineTool, ExpressionTool, Operation, StringOrDocument, Workflow,
};
use cwl_core::{from_str, load_cwl_file};
use cwl_engine::{
    ContainerEngine, LocalBackend, TaskBackend,
    create_execution_request_from_document, evaluate_exitcodes, execute,
};
use cwl_engine_storage::{StorageBackend, StoragePath};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::app::{App, AppEvent};

pub(crate) fn detect_cwl(path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    if !content.contains("cwlVersion") {
        return false;
    }
    from_str(&content).is_ok()
}

pub(crate) fn load_cwl(path: &Path) -> Result<CWLDocument> {
    load_cwl_file(path, true).with_context(|| format!("Failed to load CWL file {}", path.display()))
}

pub(crate) fn cwl_title(doc: &CWLDocument) -> String {
    match doc {
        CWLDocument::CommandLineTool(t) => t
            .id
            .clone()
            .unwrap_or_else(|| "CommandLineTool".to_string()),
        CWLDocument::Workflow(w) => w.id.clone().unwrap_or_else(|| "Workflow".to_string()),
        CWLDocument::ExpressionTool(_) => "ExpressionTool".to_string(),
        CWLDocument::Operation(_) => "Operation".to_string(),
    }
}

pub(crate) fn cwl_summary(doc: &CWLDocument) -> String {
    let version = doc.cwl_version().map_or("unknown".to_string(), |v| v.clone());
    let class = match doc {
        CWLDocument::CommandLineTool(_) => "CommandLineTool",
        CWLDocument::Workflow(_) => "Workflow",
        CWLDocument::ExpressionTool(_) => "ExpressionTool",
        CWLDocument::Operation(_) => "Operation",
    };
    format!("{} ({})", class, version)
}

pub(crate) fn cwl_inputs(doc: &CWLDocument) -> Vec<(String, String)> {
    match doc {
        CWLDocument::CommandLineTool(t) => t
            .inputs
            .iter()
            .map(|i| {
                (
                    i.id.clone().unwrap_or_default(),
                    format!("{:?}", i.r#type),
                )
            })
            .collect(),
        CWLDocument::Workflow(w) => w
            .inputs
            .iter()
            .map(|i| {
                (
                    i.id.clone().unwrap_or_default(),
                    format!("{:?}", i.r#type),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) async fn run_cwl(tab: usize, id: usize, path: PathBuf, tx: UnboundedSender<AppEvent>) {
    if let Err(e) = run_cwl_inner(tab, id, &path, &tx).await {
        let _ = tx.send(AppEvent::ProcessError {
            tab,
            id,
            error: e.to_string(),
        });
    }
}

async fn run_cwl_inner(
    tab: usize,
    id: usize,
    path: &Path,
    tx: &UnboundedSender<AppEvent>,
) -> Result<()> {
    let _ = tx.send(AppEvent::ProcessLine {
        tab,
        id,
        line: format!("Loading CWL: {}", path.display()),
    });
    let doc = load_cwl_file(path, true)?;
    let _ = tx.send(AppEvent::ProcessLine {
        tab,
        id,
        line: "Loaded CWL document".to_string(),
    });
    let base_path = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let inputs = cwl_engine::InputObject::default();

    let outputs_path = std::env::temp_dir().join(format!("rbt-cwl-outputs-{id}"));
    tokio::fs::create_dir_all(&outputs_path).await?;

    let request = create_execution_request_from_document(
        doc,
        inputs,
        base_path,
        Some(&outputs_path),
        None,
    )?;

    let storage = Arc::new(StorageBackend::new());
    let data_store = StoragePath::from_local(&std::env::temp_dir());
    let backend: Arc<dyn TaskBackend> = Arc::new(LocalBackend::new(
        ContainerEngine::Docker,
        storage,
        data_store,
    ));

    let _ = tx.send(AppEvent::ProcessLine {
        tab,
        id,
        line: format!("CWL outputs will be written to {}", outputs_path.display()),
    });
    let _ = tx.send(AppEvent::ProcessLine {
        tab,
        id,
        line: "Executing CWL document...".to_string(),
    });

    let result = execute(backend, &request, CancellationToken::new()).await;

    match result {
        Ok(result) => {
            emit_lines(tab, id, &result.stdout, tx);
            if !result.stderr.is_empty() {
                for line in result.stderr.lines() {
                    let _ = tx.send(AppEvent::ProcessLine {
                        tab,
                        id,
                        line: format!("stderr: {line}"),
                    });
                }
            }

            let status = evaluate_exitcodes(&result.exit_status, &request.specification);
            let code = match status {
                cwl_engine::EngineStatus::Success(c) => Some(c),
                cwl_engine::EngineStatus::Failure(c) => Some(c),
                cwl_engine::EngineStatus::Undefined(c) => Some(c),
            };

            let outputs: std::collections::HashMap<String, String> = result
                .outputs
                .iter()
                .map(|(k, v)| {
                    let value = serde_json::to_string_pretty(v)
                        .unwrap_or_else(|_| format!("{v:?}"));
                    (k.clone(), value)
                })
                .collect();

            let mut output_lines = String::from("CWL outputs:");
            for (k, v) in &outputs {
                output_lines.push('\n');
                output_lines.push_str(k);
                output_lines.push_str(": ");
                output_lines.push_str(v);
            }
            if !outputs.is_empty() {
                let _ = tx.send(AppEvent::ProcessLine {
                    tab,
                    id,
                    line: output_lines,
                });
            }

            let _ = tx.send(AppEvent::CwlOutputs {
                tab,
                id,
                outputs,
            });

            if let Some(code) = code {
                if code == 0 {
                    let _ = tx.send(AppEvent::ProcessLine {
                        tab,
                        id,
                        line: "CWL execution finished successfully".to_string(),
                    });
                } else {
                    let _ = tx.send(AppEvent::ProcessLine {
                        tab,
                        id,
                        line: format!("CWL execution failed with exit code {code}"),
                    });
                    let _ = tx.send(AppEvent::ProcessError {
                        tab,
                        id,
                        error: format!("Exit code {code}"),
                    });
                }
                let _ = tx.send(AppEvent::ProcessExit { tab, id, code: Some(code) });
            } else {
                let _ = tx.send(AppEvent::ProcessExit { tab, id, code });
            }
            Ok(())
        }
        Err(e) => {
            let error = format!("Execution error: {e:?}");
            let _ = tx.send(AppEvent::ProcessLine {
                tab,
                id,
                line: error.clone(),
            });
            let _ = tx.send(AppEvent::ProcessError {
                tab,
                id,
                error,
            });
            let _ = tx.send(AppEvent::ProcessExit { tab, id, code: Some(1) });
            Ok(())
        }
    }
}

fn emit_lines(tab: usize, id: usize, text: &str, tx: &UnboundedSender<AppEvent>) {
    for line in text.lines() {
        let _ = tx.send(AppEvent::ProcessLine {
            tab,
            id,
            line: line.to_string(),
        });
    }
}

pub(crate) fn format_graph(doc: &CWLDocument) -> String {
    let version = doc.cwl_version().map_or("unknown".to_string(), |v| v.clone());
    match doc {
        CWLDocument::Workflow(wf) => format_workflow(wf, &version),
        CWLDocument::CommandLineTool(t) => format_command_line_tool(t, &version),
        CWLDocument::ExpressionTool(t) => format_expression_tool(t, &version),
        CWLDocument::Operation(t) => format_operation(t, &version),
    }
}

fn format_workflow(wf: &Workflow, version: &str) -> String {
    let mut labels: Vec<String> = Vec::new();
    let mut node_id_by_name: HashMap<String, usize> = HashMap::new();

    // Workflow inputs
    for input in &wf.inputs {
        let id = labels.len() + 1;
        let name = input.id.clone().unwrap_or_default();
        let label = if name.is_empty() { "?".to_string() } else { name.clone() };
        labels.push(label);
        node_id_by_name.insert(name, id);
    }

    // Workflow steps
    for step in &wf.steps {
        let id = labels.len() + 1;
        let name = step.id.clone().unwrap_or_default();
        let run = match &step.run {
            StringOrDocument::String(path) => path.clone(),
            StringOrDocument::Document(doc) => match doc.as_ref() {
                CWLDocument::CommandLineTool(_) => "CommandLineTool".to_string(),
                CWLDocument::ExpressionTool(_) => "ExpressionTool".to_string(),
                CWLDocument::Workflow(_) => "Workflow".to_string(),
                CWLDocument::Operation(_) => "Operation".to_string(),
            },
        };
        let mut extra = String::new();
        if step.scatter.is_some() {
            extra.push_str(" [scatter]");
        }
        if let Some(w) = &step.when {
            let mut w = w
                .replace(['$', '{', '}', ';'], "")
                .split_whitespace()
                .filter(|t| *t != "return")
                .collect::<Vec<_>>()
                .join(" ");
            if w.len() > 25 {
                w = format!("{}...", &w[..25]);
            }
            if !w.is_empty() {
                extra.push_str(&format!(" [when: {w}]"));
            } else {
                extra.push_str(" [when]");
            }
        }
        let label = if name.is_empty() {
            format!("? ({run}){extra}")
        } else {
            format!("{name} ({run}){extra}")
        };
        labels.push(label);
        node_id_by_name.insert(name, id);
    }

    // Workflow outputs
    for output in &wf.outputs {
        let id = labels.len() + 1;
        let name = output.id.clone().unwrap_or_default();
        let label = if name.is_empty() { "?".to_string() } else { name.clone() };
        labels.push(label);
        node_id_by_name.insert(name, id);
    }

    let mut edge_labels: Vec<String> = Vec::new();
    let mut edges: Vec<(usize, usize, usize)> = Vec::new();

    for step in &wf.steps {
        let step_id = match step.id.as_ref().and_then(|s| node_id_by_name.get(s)) {
            Some(&id) => id,
            None => continue,
        };
        for input in &step.r#in {
            let input_name = input.id.as_deref().unwrap_or("?");
            let sources = input
                .source
                .as_ref()
                .map(|s| s.as_many())
                .unwrap_or_default();
            for source in &sources {
                let src_id = if let Some(pos) = source.find('/') {
                    let src_step = &source[..pos];
                    node_id_by_name.get(src_step).copied().unwrap_or(0)
                } else {
                    node_id_by_name.get(source).copied().unwrap_or(0)
                };
                if src_id == 0 {
                    continue;
                }
                edge_labels.push(input_name.to_string());
                edges.push((src_id, step_id, edge_labels.len() - 1));
            }
        }
    }

    for output in &wf.outputs {
        let output_id = match output.id.as_ref().and_then(|s| node_id_by_name.get(s)) {
            Some(&id) => id,
            None => continue,
        };
        let sources = output
            .output_source
            .as_ref()
            .map(|s| s.as_many())
            .unwrap_or_default();
        for source in &sources {
            let src_id = if let Some(pos) = source.find('/') {
                let src_step = &source[..pos];
                node_id_by_name.get(src_step).copied().unwrap_or(0)
            } else {
                node_id_by_name.get(source).copied().unwrap_or(0)
            };
            if src_id == 0 {
                continue;
            }
            let edge_label = if let Some(pos) = source.find('/') {
                &source[pos + 1..]
            } else {
                source.as_str()
            };
            edge_labels.push(edge_label.to_string());
            edges.push((src_id, output_id, edge_labels.len() - 1));
        }
    }

    let mut graph = Graph::new();
    graph.set_render_mode(RenderMode::Vertical);
    for (i, label) in labels.iter().enumerate() {
        graph.add_node(i + 1, label.as_str());
    }
    for (from, to, label_idx) in &edges {
        graph.add_edge(*from, *to, Some(edge_labels[*label_idx].as_str()));
    }

    let title = format!(
        "Workflow: {} ({})\n",
        wf.id.as_deref().unwrap_or("unnamed"),
        version
    );
    title + &graph.render()
}

fn format_command_line_tool(tool: &CommandLineTool, version: &str) -> String {
    let mut s = format!(
        "CommandLineTool: {} ({})\n",
        tool.id.as_deref().unwrap_or("unnamed"),
        version
    );
    s.push_str("\nInputs:\n");
    for input in &tool.inputs {
        s.push_str(&format!(
            "  {}\n",
            input.id.as_deref().unwrap_or("?")
        ));
    }
    s.push_str("\nOutputs:\n");
    for output in &tool.outputs {
        s.push_str(&format!(
            "  {}\n",
            output.id.as_deref().unwrap_or("?")
        ));
    }
    s
}

fn format_expression_tool(tool: &ExpressionTool, version: &str) -> String {
    let mut s = format!(
        "ExpressionTool: {} ({})\n",
        tool.id.as_deref().unwrap_or("unnamed"),
        version
    );
    s.push_str("\nInputs:\n");
    for input in &tool.inputs {
        s.push_str(&format!(
            "  {}\n",
            input.id.as_deref().unwrap_or("?")
        ));
    }
    s.push_str("\nOutputs:\n");
    for output in &tool.outputs {
        s.push_str(&format!(
            "  {}\n",
            output.id.as_deref().unwrap_or("?")
        ));
    }
    s
}

fn format_operation(tool: &Operation, version: &str) -> String {
    let mut s = format!(
        "Operation: {} ({})\n",
        tool.id.as_deref().unwrap_or("unnamed"),
        version
    );
    s.push_str("\nInputs:\n");
    for input in &tool.inputs {
        s.push_str(&format!(
            "  {}\n",
            input.id.as_deref().unwrap_or("?")
        ));
    }
    s.push_str("\nOutputs:\n");
    for output in &tool.outputs {
        s.push_str(&format!(
            "  {}\n",
            output.id.as_deref().unwrap_or("?")
        ));
    }
    s
}

pub(crate) fn format_results(app: &App) -> String {
    let mut s = String::from("CWL Execution Results\n\n");

    let status = if let Some(error) = &app.cwl_error {
        format!("Failed: {error}")
    } else if let Some(code) = app.cwl_exit_code {
        if code == 0 {
            "Success".to_string()
        } else {
            format!("Failed (exit code {code})")
        }
    } else {
        "Not run".to_string()
    };
    s.push_str(&format!("Status: {status}\n\n"));

    s.push_str("Outputs:\n");
    if let Some(outputs) = &app.cwl_outputs {
        if outputs.is_empty() {
            s.push_str("  (none)\n");
        } else {
            for (k, v) in outputs.iter() {
                s.push_str(&format!("  {k}:\n"));
                for line in v.lines() {
                    s.push_str(&format!("    {line}\n"));
                }
            }
        }
    } else {
        s.push_str("  (not available)\n");
    }
    s.push('\n');

    if let Some(doc) = &app.cwl_doc {
        match doc {
            CWLDocument::Workflow(wf) => {
                s.push_str("Steps:\n");
                for step in &wf.steps {
                    let id = step.id.as_deref().unwrap_or("?");
                    let run = match &step.run {
                        StringOrDocument::String(path) => path.clone(),
                        StringOrDocument::Document(doc) => match doc.as_ref() {
                            CWLDocument::CommandLineTool(_) => "CommandLineTool".to_string(),
                            CWLDocument::ExpressionTool(_) => "ExpressionTool".to_string(),
                            CWLDocument::Workflow(_) => "Workflow".to_string(),
                            CWLDocument::Operation(_) => "Operation".to_string(),
                        },
                    };
                    let mut extra = String::new();
                    if step.scatter.is_some() {
                        extra.push_str(" [scatter]");
                    }
                    if step.when.is_some() {
                        extra.push_str(" [conditional]");
                    }
                    let step_status = if app.cwl_error.is_some() {
                        "failed"
                    } else if app.cwl_exit_code == Some(0) {
                        "completed"
                    } else {
                        "unknown"
                    };
                    s.push_str(&format!("  {id} ({run}){extra} - {step_status}\n"));
                }
            }
            CWLDocument::CommandLineTool(_) => {
                s.push_str("Document type: CommandLineTool\n");
            }
            CWLDocument::ExpressionTool(_) => {
                s.push_str("Document type: ExpressionTool\n");
            }
            CWLDocument::Operation(_) => {
                s.push_str("Document type: Operation\n");
            }
        }
    } else {
        s.push_str("Steps:\n  (no document loaded)\n");
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detect_cwl_from_yaml() {
        let mut file = tempfile::NamedTempFile::with_suffix(".cwl").unwrap();
        write!(
            file,
            "{}\n",
            r#"
cwlVersion: v1.2
class: CommandLineTool
baseCommand: echo
inputs: []
outputs: []
"#
            .trim()
        )
        .unwrap();
        assert!(detect_cwl(file.path()));
    }

    #[test]
    fn detect_cwl_false_for_api() {
        let mut file = tempfile::NamedTempFile::with_suffix(".yaml").unwrap();
        write!(
            file,
            "{}\n",
            r#"
apiVersion: v1
kind: Pod
metadata:
  name: test
"#
            .trim()
        )
        .unwrap();
        assert!(!detect_cwl(file.path()));
    }

    #[test]
    fn load_complex_workflow_example() {
        let path = std::path::Path::new("example-complex.cwl");
        let doc = load_cwl(path);
        assert!(doc.is_ok(), "{doc:?}");
        let doc = doc.unwrap();
        assert!(matches!(doc, CWLDocument::Workflow(_)));
    }

    #[test]
    fn run_complex_cwl_workflow() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            run_cwl(0, 2, PathBuf::from("example-complex.cwl"), tx).await;

            let mut lines = Vec::new();
            let mut exit = None;
            let mut error = None;
            while let Some(evt) = rx.recv().await {
                match evt {
                    AppEvent::ProcessLine { line, .. } => lines.push(line),
                    AppEvent::ProcessExit { code, .. } => {
                        exit = code;
                        break;
                    }
                    AppEvent::ProcessError { error: e, .. } => {
                        error = Some(e);
                        break;
                    }
                    _ => {}
                }
            }

            assert!(error.is_none(), "CWL run failed: {error:?}");
            assert_eq!(exit, Some(0), "events: {lines:?}");
            assert!(
                lines.iter().any(|l| l.contains("Hello, Alice!")),
                "events: {lines:?}"
            );
        });
    }

    #[test]
    fn run_simple_cwl_tool() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            run_cwl(0, 1, PathBuf::from("example.cwl"), tx).await;

            let mut lines = Vec::new();
            let mut exit = None;
            let mut error = None;
            while let Some(evt) = rx.recv().await {
                match evt {
                    AppEvent::ProcessLine { line, .. } => lines.push(line),
                    AppEvent::ProcessExit { code, .. } => {
                        exit = code;
                        break;
                    }
                    AppEvent::ProcessError { error: e, .. } => {
                        error = Some(e);
                        break;
                    }
                    _ => {}
                }
            }

            assert!(error.is_none(), "CWL run failed: {error:?}");
            assert_eq!(exit, Some(0), "events: {lines:?}");
            assert!(
                lines.iter().any(|l| l.contains("Hello from CWL")),
                "events: {lines:?}"
            );
        });
    }

    #[test]
    fn format_complex_workflow_graph() {
        let path = std::path::Path::new("example-complex.cwl");
        let doc = load_cwl(path).unwrap();
        let graph = format_graph(&doc);
        assert!(graph.contains("Workflow:"));
        assert!(graph.contains("greet"));
        assert!(graph.contains("maybe_shout"));
        assert!(graph.contains("scatter"));
        assert!(graph.contains("when"));
        assert!(graph.contains("greeting"));
        assert!(graph.contains("greetings"));
        assert!(graph.contains("shouted"));
    }

    #[test]
    fn format_cwl_results_view() {
        let mut app = crate::app::App::new(Vec::new(), reqwest::Client::new());
        let path = std::path::Path::new("example-complex.cwl");
        app.cwl_doc = load_cwl(path).ok();
        app.cwl_exit_code = Some(0);
        let mut outputs = std::collections::HashMap::new();
        outputs.insert("all_greetings".to_string(), "[\"Hello, Alice!\"]".to_string());
        outputs.insert("shouted".to_string(), "[\"HELLO, ALICE!\"]".to_string());
        app.cwl_outputs = Some(outputs);

        let results = format_results(&app);
        assert!(results.contains("CWL Execution Results"));
        assert!(results.contains("Status: Success"));
        assert!(results.contains("all_greetings"));
        assert!(results.contains("shouted"));
        assert!(results.contains("greet"));
        assert!(results.contains("maybe_shout"));
        assert!(results.contains("completed"));
    }
}


