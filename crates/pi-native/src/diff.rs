use napi_derive::napi;
use similar::{ChangeTag, TextDiff};

#[napi]
pub fn generate_unified_patch(path: String, old_content: String, new_content: String, context_lines: u32) -> String {
    let diff = TextDiff::from_lines(&old_content, &new_content);
    diff.unified_diff()
        .context_radius(context_lines as usize)
        .header(&path, &path)
        .to_string()
}

#[napi(object)]
pub struct DiffStringResult {
    pub diff: String,
    pub first_changed_line: Option<u32>,
}

#[napi]
pub fn generate_diff_string(old_content: String, new_content: String, context_lines: u32) -> DiffStringResult {
    let diff = TextDiff::from_lines(&old_content, &new_content);
    let mut output = Vec::new();
    let mut first_changed_line = None;
    
    let old_lines = diff.old_slices().len();
    let new_lines = diff.new_slices().len();
    let max_line_num = std::cmp::max(old_lines, new_lines);
    let line_num_width = max_line_num.to_string().len();
    
    let context_lines = context_lines as usize;
    let ops = diff.grouped_ops(context_lines);
    
    for (i, group) in ops.iter().enumerate() {
        if i > 0 {
            output.push(format!(" {:>width$} ...", "", width = line_num_width));
        }
        
        for op in group {
            for change in diff.iter_changes(op) {
                let (old_idx, new_idx) = (change.old_index(), change.new_index());
                let val = change.value();
                let val_str = if val.ends_with("\n") { &val[..val.len()-1] } else { val };
                let val_str = if val_str.ends_with("\r") { &val_str[..val_str.len()-1] } else { val_str };
                
                match change.tag() {
                    ChangeTag::Equal => {
                        output.push(format!(" {:>width$} {}", old_idx.unwrap() + 1, val_str, width = line_num_width));
                    }
                    ChangeTag::Delete => {
                        output.push(format!("-{:>width$} {}", old_idx.unwrap() + 1, val_str, width = line_num_width));
                    }
                    ChangeTag::Insert => {
                        let new_line = new_idx.unwrap() + 1;
                        if first_changed_line.is_none() {
                            first_changed_line = Some(new_line as u32);
                        }
                        output.push(format!("+{:>width$} {}", new_line, val_str, width = line_num_width));
                    }
                }
            }
        }
    }
    
    DiffStringResult {
        diff: output.join("\n"),
        first_changed_line,
    }
}
