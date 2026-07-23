use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;

use crate::app::AppEvent;

pub(crate) async fn run_process(id: usize, command: String, log_path: PathBuf, tx: UnboundedSender<AppEvent>) {
    if let Err(e) = run_process_inner(id, command, log_path, tx.clone()).await {
        let _ = tx.send(AppEvent::ProcessError { id, error: e.to_string() });
    }
}

async fn run_process_inner(
    id: usize,
    command: String,
    log_path: PathBuf,
    tx: UnboundedSender<AppEvent>,
) -> std::io::Result<()> {
    if let Some(parent) = log_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let file = tokio::fs::File::create(&log_path).await?;
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let stderr = BufReader::new(child.stderr.take().unwrap()).lines();

    let file = Arc::new(Mutex::new(BufWriter::new(file)));
    let tx_out = tx.clone();
    let tx_err = tx.clone();

    let h1 = tokio::spawn(read_lines(id, stdout, file.clone(), tx_out));
    let h2 = tokio::spawn(read_lines(id, stderr, file.clone(), tx_err));

    let (r1, r2) = tokio::join!(h1, h2);
    r1.map_err(std::io::Error::other)??;
    r2.map_err(std::io::Error::other)??;

    let mut file = file.lock().await;
    file.flush().await?;
    drop(file);

    let status = child.wait().await?;
    let code = status.code();
    let _ = tx.send(AppEvent::ProcessExit { id, code });
    Ok(())
}

async fn read_lines<R: tokio::io::AsyncBufRead + Unpin>(
    id: usize,
    mut lines: tokio::io::Lines<R>,
    file: Arc<Mutex<BufWriter<tokio::fs::File>>>,
    tx: UnboundedSender<AppEvent>,
) -> std::io::Result<()> {
    while let Some(line) = lines.next_line().await? {
        let mut file = file.lock().await;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        drop(file);
        if tx.send(AppEvent::ProcessLine { id, line }).is_err() {
            break;
        }
    }
    Ok(())
}
