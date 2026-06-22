use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::store::open_connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Complete,
    Blocked,
}

impl GoalStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Complete => "complete",
            Self::Blocked => "blocked",
        }
    }

    pub fn is_continuable(self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "complete" => Some(Self::Complete),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

impl std::fmt::Display for GoalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalState {
    pub objective: String,
    pub status: GoalStatus,
    pub turns_completed: u32,
    pub max_turns: u32,
    pub created_at: String,
    pub updated_at: String,
}

pub fn save_goal(store_path: &Path, session_id: &str, goal: &GoalState) -> Result<()> {
    let conn = open_connection(store_path)?;
    conn.execute(
        "INSERT INTO goals (session_id, objective, status, turns_completed, max_turns, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(session_id) DO UPDATE SET
             objective = excluded.objective,
             status = excluded.status,
             turns_completed = excluded.turns_completed,
             max_turns = excluded.max_turns,
             updated_at = excluded.updated_at",
        rusqlite::params![
            session_id,
            goal.objective,
            goal.status.label(),
            goal.turns_completed,
            goal.max_turns,
            goal.created_at,
            goal.updated_at,
        ],
    )?;
    Ok(())
}

pub fn load_goal(store_path: &Path, session_id: &str) -> Result<Option<GoalState>> {
    let conn = open_connection(store_path)?;
    let result = conn
        .query_row(
            "SELECT objective, status, turns_completed, max_turns, created_at, updated_at
             FROM goals WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok(GoalState {
                    objective: row.get(0)?,
                    status: GoalStatus::from_str(&row.get::<_, String>(1)?)
                        .unwrap_or(GoalStatus::Active),
                    turns_completed: row.get(2)?,
                    max_turns: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(result)
}

pub fn delete_goal(store_path: &Path, session_id: &str) -> Result<bool> {
    let conn = open_connection(store_path)?;
    let rows_deleted = conn.execute("DELETE FROM goals WHERE session_id = ?1", [session_id])?;
    Ok(rows_deleted > 0)
}

pub fn now_utc() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // days since epoch to Y/M/D (simplified leap year calculation)
    let days = secs / 86400;
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant's date algorithms
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
