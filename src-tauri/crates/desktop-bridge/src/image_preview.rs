//! Derived previews stay beside account-scoped attachments, never at a public URL.
use std::{path::{Path,PathBuf},io::{Cursor,Write}};

pub fn path(source:&Path)->Result<PathBuf,String> {
    let target=source.with_extension("preview-v1.jpg");
    if target.exists() {
        if std::fs::symlink_metadata(&target).map_err(|_|"缩略图不可用")?.file_type().is_symlink(){return Err("缩略图路径无效".into());}
        return Ok(target);
    }
    let bytes=std::fs::read(source).map_err(|_|"图片暂不可用")?;
    let mut reader=image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|_|"图片格式无效")?;
    let mut limits=image::Limits::default();limits.max_image_width=Some(16000);limits.max_image_height=Some(16000);limits.max_alloc=Some(128*1024*1024);
    reader.limits(limits);
    let decoded=reader.decode().map_err(|_|"图片无法生成预览")?;
    let preview=decoded.thumbnail(480,480).to_rgb8();
    let mut output=Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output,70).encode_image(&preview).map_err(|_|"图片预览失败")?;
    let temp=source.with_extension(format!("{}.part",uuid::Uuid::new_v4()));
    let mut options=std::fs::OpenOptions::new();options.write(true).create_new(true);
    #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
    let result=(||{let mut f=options.open(&temp).map_err(|_|"无法保存缩略图")?;f.write_all(&output).map_err(|_|"无法保存缩略图")?;
        std::fs::rename(&temp,&target).map_err(|_|"无法保存缩略图")?;Ok(target)})();
    if temp.exists(){let _=std::fs::remove_file(temp);}result
}
#[cfg(test)]mod tests {
    #[test]fn preview_is_small_cached_and_never_replaces_original(){
        let root=tempfile::tempdir().unwrap();let source=root.path().join("image");
        let mut bytes=std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1800,1200).write_to(&mut bytes,image::ImageFormat::Png).unwrap();
        std::fs::write(&source,bytes.get_ref()).unwrap();let preview=super::path(&source).unwrap();
        let decoded=image::load_from_memory(&std::fs::read(&preview).unwrap()).unwrap();
        assert_eq!((decoded.width(),decoded.height()),(480,320));
        assert_eq!(std::fs::read(source).unwrap(),*bytes.get_ref());
        assert_eq!(super::path(&root.path().join("image")).unwrap(),preview);
    }
}
