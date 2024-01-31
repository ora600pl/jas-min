use std::{process::{Command, Stdio}, os::unix::prelude::FileExt};
use execute::Execute;
use serde::{Deserialize, Serialize};
use linked_hash_map::LinkedHashMap;
use std::error::Error;
use std::fmt;
use std::collections::HashMap;
use base64::{Engine as _, engine::general_purpose};
use std::fs;

#[derive(Debug)]
pub struct OstaskError {
    details: String
}

impl OstaskError {
    fn new(msg: &str) -> OstaskError {
        OstaskError{details: msg.to_string()}
    }
}

impl fmt::Display for OstaskError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,"{}",self.details)
    }
}

impl Error for OstaskError {
    fn description(&self) -> &str {
        &self.details
    }
}

fn host_op(cmd: &mut Command, success_msg: String) -> String {
    cmd.stderr(Stdio::piped());
    cmd.stdout(Stdio::piped());
    let out = cmd.execute_output();
    let out = match out {
        Ok(o) => o,
        Err(e) => return format!("{{\"status\": \"ERROR\", \"msg\": \"{}\"}}", e.to_string()),
    };

    let msg = String::from_utf8(out.stdout).unwrap();
    let err_msg = String::from_utf8(out.stderr).unwrap();

    if out.status.success() {
        if success_msg == "LIST" {
            return format!("{{\"status\": \"OK\", \"msg\": \"{}\"}}", msg);
        } else {
            return format!("{{\"status\": \"OK\", \"msg\": \"{}\"}}", success_msg);
        }
        
    } else {
        return format!("{{\"status\": \"ERROR\", \"msg\": \"{}\"}}", err_msg);
    }

    "?".to_string()
}

fn create_directory(path: Vec<String>) -> String {
    let mut cmd = Command::new("mkdir");
    cmd.arg("-p");
    let mut dir_path = path[0].to_string();
    if &dir_path[0..1] == "/" {
        dir_path = path[0][1..].to_string();
    }
    cmd.arg(&dir_path);
    host_op(&mut cmd, format!("Directory {} created properly", &dir_path))
}

fn remove_directory(path: Vec<String>) -> String {
    let mut cmd = Command::new("rm");
    cmd.arg("-rf");
    let mut dir_path = path[0].to_string();
    if &dir_path[0..1] == "/" {
        dir_path = path[0][1..].to_string();
    }
    cmd.arg(&dir_path);
    host_op(&mut cmd, format!("Directory {} removed properly", &dir_path))
}

fn write_file_to_disk(params: Vec<String>) -> String {
    let mut file_path = params[0].to_string();
    let file_b64 = &params[1];
    
    let file_data = general_purpose::STANDARD.decode(file_b64).unwrap();
    match fs::write(&file_path, file_data) {
        Ok(_) => return format!("{{\"status\": \"OK\", \"msg\": \"file {} created successfully\"}}", &file_path),
        Err(e) => return format!("{{\"status\": \"ERROR\", \"msg\": \"{}\"}}", e.to_string())
    };
}

fn list_files(path: Vec<String>) -> String {
    let mut dir_path = path[0].to_string();
    if &dir_path[0..1] == "/" {
        dir_path = path[0][1..].to_string();
    }

    #[derive(Default,Serialize, Deserialize)]
    struct FileData {
        file_name: String,
        file_type: String,
    }
    #[derive(Default,Serialize, Deserialize)]
    struct FileList {
        status: String,
        files: Vec<FileData>,
    }
    let mut file_list: FileList = FileList::default();
    
    if let Ok(entries) = fs::read_dir(&dir_path) {
        for entry in entries {
            if let Ok(entry) = entry {
                if let Ok(file_meta) = entry.metadata() {
                    //println!("{} {:?}", entry.path().display(), file_meta);
                    let mut file_type = "file".to_string();
                    if file_meta.is_dir() {
                        file_type = "directory".to_string();
                    }
                    file_list.files.push(FileData { file_name: entry.path().display().to_string(), file_type: file_type })
                } else {
                    return format!("{{\"status\": \"ERROR\", \"msg\": \"I can't read metadata for: {}\"}}", &dir_path);
                }
            }
        }
        file_list.status = "OK".to_string();
        let fl = serde_json::to_string_pretty(&file_list).unwrap();
        return fl;
    }
    else {
        return format!("{{\"status\": \"ERROR\", \"msg\": \"I can't read directory {}\"}}", &dir_path);
    }

}

pub fn execute_command(request: String) -> String {
    let mut commands: HashMap<String, fn(Vec<String>)->String> = HashMap::new();
    commands.insert("create_directory".to_string(), create_directory);
    commands.insert("remove_directory".to_string(), remove_directory);
    commands.insert("write_file_to_disk".to_string(), write_file_to_disk);
    commands.insert("list_files".to_string(), list_files);

    println!("{}", &request);
    let cmd_data: Result<LinkedHashMap<String, Vec<String>>, serde_json::Error> = serde_json::from_str(&request);
    let cmd_data: LinkedHashMap<String, Vec<String>> = match cmd_data {
        Ok(c) => c,
        Err(e) => return format!("{{\"status\": \"ERROR\", \"msg\": \"{}\"}}", e.to_string()),
    };

    for (command, args) in &cmd_data {
        println!("{}{:?}", command, args);        
        if commands.contains_key(command) {
            let f = commands.get(command).unwrap();
            return f(args.to_vec());
        } else {
            return "{{\"status\": \"ERROR\", \"msg\": \"unknown command\"}}".to_string();
        }
    }
    "?".to_string()
}