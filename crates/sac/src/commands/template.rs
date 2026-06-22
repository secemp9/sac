use std::path::Path;

const SHELL_OUTPUT_MAX: usize = 32_768;

pub fn expand_command_template(
    template: &str,
    arguments: &str,
    working_directory: &Path,
) -> String {
    let mut result = template.to_string();
    let has_arguments_var = result.contains("$ARGUMENTS");
    let max_pos = find_max_positional(&result);
    let had_variables = has_arguments_var || max_pos > 0;

    // 1. Positional substitution
    if max_pos > 0 {
        let positionals = shell_split(arguments);
        for i in 1..max_pos {
            let placeholder = format!("${}", i);
            let value = positionals.get(i - 1).map(|s| s.as_str()).unwrap_or("");
            result = result.replace(&placeholder, value);
        }
        // Last positional captures all remaining tokens
        let last_placeholder = format!("${}", max_pos);
        let remainder = if positionals.len() >= max_pos {
            positionals[max_pos - 1..].join(" ")
        } else {
            String::new()
        };
        result = result.replace(&last_placeholder, &remainder);
    }

    // 2. $ARGUMENTS substitution
    if has_arguments_var {
        result = result.replace("$ARGUMENTS", arguments);
    }

    // 3. Append args if no variables were present
    if !had_variables && !arguments.is_empty() {
        result.push_str("\n\n");
        result.push_str(arguments);
    }

    // 4. @file inclusions
    result = expand_file_inclusions(&result, working_directory);

    // 5. !`cmd` shell injections
    result = expand_shell_injections(&result, working_directory);

    result
}

fn find_max_positional(template: &str) -> usize {
    let mut max = 0usize;
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i > start {
                if let Ok(n) = std::str::from_utf8(&bytes[start..i])
                    .unwrap_or("")
                    .parse::<usize>()
                {
                    max = max.max(n);
                }
            }
        } else {
            i += 1;
        }
    }
    max
}

fn shell_split(input: &str) -> Vec<String> {
    let input = input.trim();
    if input.is_empty() {
        return Vec::new();
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            c if c.is_whitespace() && !in_single_quote && !in_double_quote => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '\\' if !in_single_quote => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn expand_file_inclusions(template: &str, working_directory: &Path) -> String {
    let mut result = String::with_capacity(template.len());
    let mut last_end = 0;
    let bytes = template.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'@' {
            // Check word boundary: must be at start or preceded by whitespace
            let at_boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
            if at_boundary {
                let path_start = i + 1;
                let mut path_end = path_start;
                // Collect non-whitespace chars after @
                while path_end < len && !bytes[path_end].is_ascii_whitespace() {
                    path_end += 1;
                }
                if path_end > path_start {
                    let file_path =
                        std::str::from_utf8(&bytes[path_start..path_end]).unwrap_or("");
                    // Skip email-like patterns (contains @ before this position)
                    // and skip if path starts with non-path chars
                    if !file_path.is_empty()
                        && !file_path.starts_with('@')
                        && (file_path.contains('/') || file_path.contains('.'))
                    {
                        let resolved = working_directory.join(file_path);
                        result.push_str(&template[last_end..i]);

                        match std::fs::read_to_string(&resolved) {
                            Ok(content) => {
                                let trimmed = content.trim();
                                result.push_str(&format!(
                                    "<file path=\"{}\">\n{}\n</file>",
                                    file_path, trimmed
                                ));
                            }
                            Err(_) => {
                                // Leave the @reference as-is if file not found
                                result.push_str(&template[i..path_end]);
                            }
                        }
                        last_end = path_end;
                        i = path_end;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }

    result.push_str(&template[last_end..]);
    result
}

fn expand_shell_injections(template: &str, working_directory: &Path) -> String {
    let mut result = template.to_string();

    // Process !`...` patterns iteratively (from left to right)
    loop {
        let Some(start) = result.find("!`") else {
            break;
        };
        let cmd_start = start + 2;
        let Some(end_offset) = result[cmd_start..].find('`') else {
            break;
        };
        let cmd_end = cmd_start + end_offset;
        let command = result[cmd_start..cmd_end].to_string();
        let full_end = cmd_end + 1;

        let replacement = execute_shell_command(&command, working_directory);
        result.replace_range(start..full_end, &replacement);
    }

    result
}

fn execute_shell_command(command: &str, working_directory: &Path) -> String {
    use std::process::Command;

    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(working_directory)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let trimmed = stdout.trim();
            if trimmed.is_empty() {
                String::new()
            } else if trimmed.len() > SHELL_OUTPUT_MAX {
                format!(
                    "{}\n[... truncated at {} bytes]",
                    &trimmed[..SHELL_OUTPUT_MAX],
                    SHELL_OUTPUT_MAX
                )
            } else {
                trimmed.to_string()
            }
        }
        Err(_) => format!("[Error: failed to execute `{}`]", command),
    }
}
