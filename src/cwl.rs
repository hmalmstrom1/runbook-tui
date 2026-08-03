use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use cwl_core::documents::CWLDocument;
use cwl_core::{from_str, load_cwl_file};
use cwl_engine::{
    ContainerEngine, LocalBackend, TaskBackend,
    create_execution_request_from_document, evaluate_exitcodes, execute,
};
use cwl_engine_storage::{StorageBackend, StoragePath};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::app::AppEvent;

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
    let doc = load_cwl_file(path, true)?;
    let base_path = path.parent().unwrap_or(Path::new("."));
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

    let result = execute(backend, &request, CancellationToken::new()).await?;

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

    if !result.outputs.is_empty() {
        let _ = tx.send(AppEvent::ProcessLine {
            tab,
            id,
            line: format!("Outputs: {:?}", result.outputs),
        });
    }

    let _ = tx.send(AppEvent::ProcessExit { tab, id, code });
    Ok(())
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


