use super::*;
use super::template::expand_command_template;

#[derive(Clone)]
pub struct CommandRegistry {
    commands: Arc<HashMap<String, CommandRecord>>,
}

impl CommandRegistry {
    pub fn load(workspace_dir: Option<&Path>) -> Option<Arc<Self>> {
        let sources = discover_command_sources(workspace_dir);
        let mut commands = HashMap::new();

        for source in sources {
            let files = discover_command_files(&source.root);

            for (name, path) in files {
                if commands.contains_key(&name) {
                    tracing::debug!(
                        command = %name,
                        path = %path.display(),
                        "command shadowed by higher-precedence definition"
                    );
                    continue;
                }

                match parse_command_file(&path) {
                    Ok(Some(parsed)) => {
                        if parsed.model.is_some() {
                            tracing::debug!(
                                command = %name,
                                model = ?parsed.model,
                                "command specifies model override (ignored in sac)"
                            );
                        }
                        commands.insert(
                            name.clone(),
                            CommandRecord {
                                name: name.clone(),
                                description: parsed.description,
                                agent: parsed.agent,
                                model: parsed.model,
                                subtask: parsed.subtask,
                                template: parsed.template,
                                source_path: path,
                            },
                        );
                    }
                    Ok(None) => {
                        tracing::debug!(
                            path = %path.display(),
                            "command file skipped (empty or invalid)"
                        );
                    }
                    Err(error) => {
                        eprintln!(
                            "Command file '{}' has errors and will be skipped: {:#}",
                            path.display(),
                            error
                        );
                    }
                }
            }
        }

        if commands.is_empty() {
            return None;
        }

        tracing::info!(count = commands.len(), "loaded custom commands");
        Some(Arc::new(Self {
            commands: Arc::new(commands),
        }))
    }

    pub fn has_command(&self, name: &str) -> bool {
        self.commands.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<&CommandRecord> {
        self.commands.get(name)
    }

    pub fn catalog_entries(&self) -> Vec<CommandCatalogEntry> {
        let mut entries: Vec<_> = self
            .commands
            .values()
            .map(|cmd| CommandCatalogEntry {
                name: cmd.name.clone(),
                description: cmd.description.clone(),
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    pub fn expand(
        &self,
        name: &str,
        args: &str,
        working_directory: &Path,
    ) -> Option<String> {
        let record = self.commands.get(name)?;

        let expanded = expand_command_template(&record.template, args, working_directory);

        // Wrap in structured envelope for display roundtripping
        let mut prompt = format!(
            "# /{}: Custom Command\n\nArguments:\n{}\n\n",
            name,
            if args.is_empty() { "(none)" } else { args }
        );

        if let Some(agent) = &record.agent {
            prompt.push_str(&format!(
                "[Skill hint: consider activating the \"{}\" skill if available.]\n\n",
                agent
            ));
        }

        prompt.push_str(&expanded);
        Some(prompt)
    }
}
