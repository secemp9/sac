use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Serialize)]
pub struct Task {
    pub id: String,
    pub container_name: String,
    pub status: TaskStatus,
    pub prompt: String,
    pub output: Option<String>,
    pub branch: Option<String>,
    pub parent_task_id: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
}

impl TaskStatus {
    fn as_str(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
    fn from_str(s: &str) -> Self {
        match s {
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Running,
        }
    }
}

pub type TaskStore = Arc<Mutex<Connection>>;

pub fn open_store() -> Result<TaskStore> {
    let path = std::env::var("NAC_DB_PATH").unwrap_or_else(|_| "nac.db".to_string());
    let conn = Connection::open(&path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            container_name TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'running',
            prompt TEXT NOT NULL,
            output TEXT,
            branch TEXT,
            parent_task_id TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            completed_at TEXT
        )",
    )?;
    Ok(Arc::new(Mutex::new(conn)))
}

pub async fn insert(store: &TaskStore, task: &Task) -> Result<()> {
    let db = store.lock().await;
    db.execute(
        "INSERT INTO tasks (id, container_name, status, prompt, output, branch, parent_task_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (&task.id, &task.container_name, task.status.as_str(), &task.prompt, &task.output, &task.branch, &task.parent_task_id, &task.created_at),
    )?;
    Ok(())
}

fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        container_name: row.get(1)?,
        status: TaskStatus::from_str(&row.get::<_, String>(2)?),
        prompt: row.get(3)?,
        output: row.get(4)?,
        branch: row.get(5)?,
        parent_task_id: row.get(6)?,
        created_at: row.get(7)?,
        completed_at: row.get(8)?,
    })
}

pub async fn get(store: &TaskStore, id: &str) -> Result<Option<Task>> {
    let db = store.lock().await;
    let mut stmt = db.prepare("SELECT id, container_name, status, prompt, output, branch, parent_task_id, created_at, completed_at FROM tasks WHERE id = ?1")?;
    let mut rows = stmt.query([id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_task(row)?)),
        None => Ok(None),
    }
}

pub async fn list(store: &TaskStore) -> Result<Vec<Task>> {
    let db = store.lock().await;
    let mut stmt = db.prepare("SELECT id, container_name, status, prompt, output, branch, parent_task_id, created_at, completed_at FROM tasks ORDER BY created_at DESC LIMIT 100")?;
    let mut rows = stmt.query([])?;
    let mut tasks = Vec::new();
    while let Some(row) = rows.next()? {
        tasks.push(row_to_task(row)?);
    }
    Ok(tasks)
}

pub async fn update_completed(
    store: &TaskStore,
    id: &str,
    output: &str,
    branch: Option<&str>,
) -> Result<()> {
    let db = store.lock().await;
    db.execute(
        "UPDATE tasks SET status = 'completed', output = ?1, branch = ?2, completed_at = datetime('now') WHERE id = ?3",
        (output, branch, id),
    )?;
    Ok(())
}

pub async fn update_failed(store: &TaskStore, id: &str, output: &str) -> Result<()> {
    let db = store.lock().await;
    db.execute(
        "UPDATE tasks SET status = 'failed', output = ?1, completed_at = datetime('now') WHERE id = ?2",
        (output, id),
    )?;
    Ok(())
}
