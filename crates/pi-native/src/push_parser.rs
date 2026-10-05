use std::collections::HashMap;
use napi::Env;
use napi_derive::napi;
use pi_json::{Analysis, Decoded, Step};
use serde_json::{json, Value};

#[napi(js_name = "StreamingJsonParser")]
pub struct StreamingJsonParser {
    text: String,
    prev_skeleton: String,
    prev_patches: HashMap<String, String>,
}

#[napi]
impl StreamingJsonParser {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            text: String::new(),
            prev_skeleton: String::new(),
            prev_patches: HashMap::new(),
        }
    }

    #[napi]
    pub fn append(&mut self, env: Env, chunk: String) -> napi::Result<String> {
        self.text.push_str(&chunk);
        
        let mut ops = Vec::new();
        let analysis = pi_json::analyze(&self.text);
        
        match analysis {
            Analysis::Fallback => {
                ops.push(json!([2])); // Op 2: Fallback
            }
            Analysis::Complete | Analysis::Partial { .. } => {
                let (skeleton, patches) = match analysis {
                    Analysis::Complete => (self.text.clone(), vec![]),
                    Analysis::Partial { skeleton, patches } => (skeleton, patches),
                    _ => unreachable!(),
                };
                
                let mut active_paths = Vec::new();
                for patch in &patches {
                    let mut path_arr = Vec::new();
                    for step in &patch.path {
                        match step {
                            Step::Key(k) => path_arr.push(json!(k)),
                            Step::Index(idx) => path_arr.push(json!(idx)),
                        }
                    }
                    active_paths.push(json!(path_arr));
                }

                if skeleton != self.prev_skeleton {
                    ops.push(json!([0, skeleton, active_paths])); // Op 0: UpdateSkeleton
                    self.prev_skeleton = skeleton;
                }
                
                let mut current_patches = HashMap::new();
                for patch in patches {
                    let mut path_arr = Vec::new();
                    let mut path_str_key = String::new(); // For HashMap key
                    for (i, step) in patch.path.iter().enumerate() {
                        if i > 0 { path_str_key.push('.'); }
                        match step {
                            Step::Key(k) => {
                                path_arr.push(json!(k));
                                path_str_key.push_str(k);
                            },
                            Step::Index(idx) => {
                                path_arr.push(json!(idx));
                                path_str_key.push_str(&idx.to_string());
                            },
                        }
                    }
                    
                    let decoded = pi_json::decode_string(patch.raw).unwrap();
                    let text = match decoded {
                        Decoded::Utf8(t) => t,
                        Decoded::Utf16(units) => String::from_utf16(&units).unwrap(),
                    };
                    
                    if let Some(prev_text) = self.prev_patches.get(&path_str_key) {
                        if text.len() > prev_text.len() && text.starts_with(prev_text) {
                            let suffix = &text[prev_text.len()..];
                            ops.push(json!([1, path_arr, suffix])); // Op 1: AppendString
                        } else if text != *prev_text {
                            ops.push(json!([3, path_arr, text])); // Op 3: SetString
                        }
                    } else {
                        ops.push(json!([3, path_arr, text])); // Op 3: SetString
                    }
                    current_patches.insert(path_str_key, text);
                }
                self.prev_patches = current_patches;
            }
        }
        
        Ok(serde_json::to_string(&ops).unwrap())
    }
}
