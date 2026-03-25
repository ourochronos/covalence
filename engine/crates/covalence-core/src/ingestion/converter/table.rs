//! Markdown table linearization utilities.
//!
//! Converts pipe-delimited tables into natural language sentences
//! for better NER/embedding downstream.

/// Linearize markdown pipe tables into natural language sentences.
///
/// Converts tables like:
/// ```text
/// | Model | Developer | Year |
/// |-------|-----------|------|
/// | GraphRAG | Microsoft | 2024 |
/// ```
/// Into:
/// ```text
/// Model: GraphRAG, Developer: Microsoft, Year: 2024.
/// ```
///
/// Preserves all non-table content unchanged. Tables without headers
/// use generic "Column 1", "Column 2", etc.
pub fn linearize_tables(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut result = String::with_capacity(markdown.len());
    let mut i = 0;

    while i < lines.len() {
        // Detect a table: a pipe-delimited row followed by a
        // separator row (pipes + dashes/colons).
        if is_table_row(lines[i]) && i + 1 < lines.len() && is_separator_row(lines[i + 1]) {
            let headers = parse_table_row(lines[i]);
            i += 2; // skip header + separator

            // Process data rows.
            while i < lines.len() && is_table_row(lines[i]) {
                let cells = parse_table_row(lines[i]);
                let pairs: Vec<String> = headers
                    .iter()
                    .zip(cells.iter())
                    .filter(|(_, v)| !v.is_empty())
                    .map(|(h, v)| format!("{h}: {v}"))
                    .collect();
                if !pairs.is_empty() {
                    result.push_str(&pairs.join(", "));
                    result.push_str(".\n");
                }
                i += 1;
            }
        } else if is_table_row(lines[i]) && !is_separator_row(lines[i]) {
            // Table without headers — use generic column names.
            // Peek ahead to see if multiple pipe rows follow.
            let first_row = parse_table_row(lines[i]);
            let col_count = first_row.len();

            // Check if next line is also a table row (headerless table).
            if i + 1 < lines.len() && is_table_row(lines[i + 1]) && !is_separator_row(lines[i + 1])
            {
                let headers: Vec<String> = (1..=col_count).map(|n| format!("Column {n}")).collect();

                // Linearize all consecutive rows including the first.
                while i < lines.len() && is_table_row(lines[i]) && !is_separator_row(lines[i]) {
                    let cells = parse_table_row(lines[i]);
                    let pairs: Vec<String> = headers
                        .iter()
                        .zip(cells.iter())
                        .filter(|(_, v)| !v.is_empty())
                        .map(|(h, v)| format!("{h}: {v}"))
                        .collect();
                    if !pairs.is_empty() {
                        result.push_str(&pairs.join(", "));
                        result.push_str(".\n");
                    }
                    i += 1;
                }
            } else {
                // Single pipe row — not really a table, keep as-is.
                result.push_str(lines[i]);
                result.push('\n');
                i += 1;
            }
        } else {
            result.push_str(lines[i]);
            result.push('\n');
            i += 1;
        }
    }

    // Trim trailing newline added by line-by-line processing if
    // the original didn't end with one.
    if !markdown.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Check if a line looks like a markdown table row (starts/ends with `|`
/// or contains at least 2 `|` characters).
pub(crate) fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Must have at least 2 pipe characters to be a table row.
    trimmed.matches('|').count() >= 2
}

/// Check if a line is a table separator row (only `|`, `-`, `:`, and
/// spaces).
pub(crate) fn is_separator_row(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || !trimmed.contains('-') {
        return false;
    }
    trimmed
        .chars()
        .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
}

/// Parse a pipe-delimited table row into trimmed cell values.
pub(crate) fn parse_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    // Strip leading and trailing pipes.
    let without_prefix = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let inner = without_prefix.strip_suffix('|').unwrap_or(without_prefix);
    inner
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}
