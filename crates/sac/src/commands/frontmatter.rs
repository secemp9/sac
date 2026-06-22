use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct CommandFrontmatter {
    pub(super) description: Option<String>,
    pub(super) agent: Option<String>,
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) subtask: Option<bool>,
}

pub(super) struct ParsedCommandFile {
    pub(super) description: String,
    pub(super) agent: Option<String>,
    pub(super) model: Option<String>,
    pub(super) subtask: bool,
    pub(super) template: String,
}

pub(super) fn parse_command_file(path: &Path) -> Result<Option<ParsedCommandFile>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read command file '{}'", path.display()))?;

    let (frontmatter, template) = match split_frontmatter(&raw) {
        Some((fm, body)) => {
            let frontmatter: CommandFrontmatter = match serde_yaml::from_str(fm) {
                Ok(fm) => fm,
                Err(error) => {
                    eprintln!(
                        "Command file '{}' has invalid frontmatter and will be skipped: {:#}",
                        path.display(),
                        error
                    );
                    return Ok(None);
                }
            };
            (frontmatter, body.trim().to_string())
        }
        None => {
            // No frontmatter -- treat entire file as template
            let template = raw.trim().to_string();
            let frontmatter = CommandFrontmatter {
                description: None,
                agent: None,
                model: None,
                subtask: None,
            };
            (frontmatter, template)
        }
    };

    if template.is_empty() {
        eprintln!(
            "Command file '{}' has an empty template body and will be skipped",
            path.display()
        );
        return Ok(None);
    }

    Ok(Some(ParsedCommandFile {
        description: frontmatter
            .description
            .map(|d| d.trim().to_string())
            .unwrap_or_default(),
        agent: frontmatter.agent.map(|a| a.trim().to_string()),
        model: frontmatter.model.map(|m| m.trim().to_string()),
        subtask: frontmatter.subtask.unwrap_or(false),
        template,
    }))
}

fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.strip_prefix("---\n")?;
    if let Some(end) = rest.find("\n---\n") {
        let frontmatter = &rest[..end];
        let body = &rest[end + 5..];
        return Some((frontmatter, body));
    }
    if let Some(end) = rest.find("\n---") {
        let after = &rest[end + 4..];
        if after.is_empty() || after == "\n" {
            let frontmatter = &rest[..end];
            return Some((frontmatter, ""));
        }
    }
    None
}
