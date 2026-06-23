use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::store::open_connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Complete,
    Blocked,
    /// Set when session-level usage limit is exceeded during an active goal.
    UsageLimited,
    /// Set when the goal's token budget is exhausted. Terminal — only the user
    /// can raise the budget.
    BudgetLimited,
}

impl GoalStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Complete => "complete",
            Self::Blocked => "blocked",
            Self::UsageLimited => "usage_limited",
            Self::BudgetLimited => "budget_limited",
        }
    }

    pub fn is_continuable(self) -> bool {
        matches!(self, Self::Active)
    }

    /// Returns `true` for statuses that represent a truly finished state
    /// from which the goal will never auto-resume.  Matches Codex semantics:
    /// only `Complete` and `BudgetLimited` are terminal.
    ///
    /// `Blocked` and `UsageLimited` are NOT terminal — they are resumable
    /// states where the user (or system) can clear the condition and
    /// continue the goal.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::BudgetLimited)
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "complete" => Some(Self::Complete),
            "blocked" => Some(Self::Blocked),
            "usage_limited" => Some(Self::UsageLimited),
            "budget_limited" => Some(Self::BudgetLimited),
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
    /// Unique identifier for this goal instance.  Used for optimistic
    /// concurrency control: accounting operations carry the expected
    /// `goal_id` and skip the write when the stored id differs (the goal
    /// was replaced between read and write).
    pub goal_id: String,
    pub objective: String,
    pub status: GoalStatus,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub token_budget: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Generate a new random goal id (UUID v4).
pub fn new_goal_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn save_goal(store_path: &Path, session_id: &str, goal: &GoalState) -> Result<()> {
    let conn = open_connection(store_path)?;
    conn.execute(
        "INSERT INTO goals (session_id, goal_id, objective, status, tokens_used, time_used_seconds, token_budget, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(session_id) DO UPDATE SET
             goal_id = excluded.goal_id,
             objective = excluded.objective,
             status = excluded.status,
             tokens_used = excluded.tokens_used,
             time_used_seconds = excluded.time_used_seconds,
             token_budget = excluded.token_budget,
             updated_at = excluded.updated_at",
        rusqlite::params![
            session_id,
            goal.goal_id,
            goal.objective,
            goal.status.label(),
            goal.tokens_used,
            goal.time_used_seconds,
            goal.token_budget,
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
            "SELECT goal_id, objective, status, tokens_used, time_used_seconds, token_budget, created_at, updated_at
             FROM goals WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok(GoalState {
                    goal_id: row.get::<_, String>(0).unwrap_or_default(),
                    objective: row.get(1)?,
                    status: GoalStatus::from_str(&row.get::<_, String>(2)?)
                        .unwrap_or(GoalStatus::Active),
                    tokens_used: row.get(3)?,
                    time_used_seconds: row.get(4)?,
                    token_budget: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .optional()?;
    Ok(result)
}

/// Result of accounting goal usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountingOutcome {
    /// Usage was recorded; goal remains within budget (or has no budget).
    WithinBudget,
    /// Usage was recorded; goal now exceeds its token budget.
    BudgetExceeded,
    /// The accounting was skipped because the stored `goal_id` did not
    /// match the expected value (the goal was replaced between the
    /// snapshot and the write).
    Skipped,
}

/// Atomically increment `tokens_used` and `time_used_seconds` for the
/// session's goal, returning whether the budget is now exceeded.
///
/// When `expected_goal_id` is `Some`, the update is conditional: the row
/// is only modified when the stored `goal_id` matches.  If it does not
/// match the goal was replaced between the snapshot and the write, and
/// `AccountingOutcome::Skipped` is returned to avoid charging the wrong
/// goal instance.  This mirrors Codex's optimistic concurrency check in
/// `account_thread_goal_usage`.
pub fn account_goal_usage(
    store_path: &Path,
    session_id: &str,
    token_delta: i64,
    time_delta_seconds: i64,
    expected_goal_id: Option<&str>,
) -> Result<AccountingOutcome> {
    let conn = open_connection(store_path)?;
    let now = now_utc();

    let rows_affected = if let Some(expected_id) = expected_goal_id {
        conn.execute(
            "UPDATE goals
             SET tokens_used = tokens_used + ?1,
                 time_used_seconds = time_used_seconds + ?2,
                 updated_at = ?3
             WHERE session_id = ?4 AND goal_id = ?5",
            rusqlite::params![token_delta, time_delta_seconds, now, session_id, expected_id],
        )?
    } else {
        conn.execute(
            "UPDATE goals
             SET tokens_used = tokens_used + ?1,
                 time_used_seconds = time_used_seconds + ?2,
                 updated_at = ?3
             WHERE session_id = ?4",
            rusqlite::params![token_delta, time_delta_seconds, now, session_id],
        )?
    };

    if rows_affected == 0 {
        // Either the goal does not exist, or the goal_id didn't match
        // (the goal was replaced). In both cases, skip accounting.
        return Ok(AccountingOutcome::Skipped);
    }

    // Read back the current state to check budget
    let (tokens_used, token_budget): (i64, Option<i64>) = conn.query_row(
        "SELECT tokens_used, token_budget FROM goals WHERE session_id = ?1",
        [session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    match token_budget {
        Some(budget) if tokens_used >= budget => Ok(AccountingOutcome::BudgetExceeded),
        _ => Ok(AccountingOutcome::WithinBudget),
    }
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
