//! Read bounded task workspace files. Canonical checks keep symlinks inside the task root.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{fs, io::{Read,Seek,SeekFrom},path::Path};
pub fn read(cwd:&Path,params:&Value)->Result<Value,String> {
    let raw=params["path"].as_str().filter(|p|!p.is_empty()&&p.len()<=4096&&!p.contains('\0')).ok_or("文件路径无效")?;
    let root=cwd.canonicalize().map_err(|_|"任务目录不可用")?;
    let input=Path::new(raw);let path=if input.is_absolute(){input.to_path_buf()}else{root.join(input)};
    let path=path.canonicalize().map_err(|_|"文件不存在或无法访问")?;
    if !path.starts_with(&root){return Err("此文件位于任务目录之外，请在电脑查看".into());}
    let mut file=fs::File::open(&path).map_err(|_|"无法读取文件")?;
    let meta=file.metadata().map_err(|_|"无法读取文件信息")?;
    if !meta.is_file()||meta.len()>5*1024*1024 {return Err("仅支持查看不超过 5 MB 的文件".into());}
    let offset=params["offset"].as_u64().filter(|n|*n<=meta.len()).ok_or("文件偏移无效")?;
    file.seek(SeekFrom::Start(offset)).map_err(|_|"文件读取失败")?;
    let mut bytes=vec![0;(meta.len()-offset).min(49152) as usize];
    file.read_exact(&mut bytes).map_err(|_|"文件读取未完成")?;
    let modified=meta.modified().ok().and_then(|t|t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d|d.as_nanos().to_string()).unwrap_or_default();
    let version=format!("{}:{modified}",meta.len());
    if params["version"].as_str().is_some_and(|v|v!=version) {return Err("文件正在变化，请重新打开".into());}
    Ok(json!({"name":path.file_name().and_then(|v|v.to_str()).unwrap_or("file"),"total":meta.len(),"offset":offset+bytes.len() as u64,"version":version,"data":STANDARD.encode(bytes)}))
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn task_files_deny_escape_and_detect_revision_changes(){
        let d=tempfile::tempdir().unwrap();let file=d.path().join("result.txt");fs::write(&file,"hello").unwrap();
        let p=json!({"path":"result.txt","offset":0});let read=read(d.path(),&p).unwrap();assert_eq!(read["data"],"aGVsbG8=");
        assert!(super::read(d.path(),&json!({"path":"../outside","offset":0})).is_err());
        fs::write(&file,"changed").unwrap();assert!(super::read(d.path(),&json!({"path":"result.txt","offset":0,"version":read["version"]})).is_err());
        #[cfg(unix)] {let outside=tempfile::NamedTempFile::new().unwrap();std::os::unix::fs::symlink(outside.path(),d.path().join("escape")).unwrap();assert!(super::read(d.path(),&json!({"path":"escape","offset":0})).is_err());}
    }
}
