use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Serialize)]
pub struct Task {
    pub id: String,
    pub status: TaskStatus,
    pub prompt: String,
    pub output: Option<String>,
    pub branch: Option<String>,
    pub parent_task_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
}

pub type TaskStore = Arc<Mutex<HashMap<String, Task>>>;

pub fn new_store() -> TaskStore {
    Arc::new(Mutex::new(HashMap::new()))
}
