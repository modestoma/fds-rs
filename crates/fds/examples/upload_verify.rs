//! 文件上传、HEAD 长度检查、流式 MD5 核对与预签名链接。
//! 环境变量：FDS_ENDPOINT / FDS_ACCESS_KEY_ID / FDS_ACCESS_KEY_SECRET。
//! 运行：cargo run --example upload_verify -- BUCKET UNIQUE_KEY LOCAL_FILE
//! stdout 包含临时访问 URL；请交给受控接收方，不写入公开日志。
use fds::object::{UploadBody, UserMetadata};
use fds::Client;
use http::Method;
use md5::{Digest, Md5};
use std::{error::Error, path::Path, time::Duration};
use tokio::io::AsyncReadExt;

async fn file_digest(path: &Path) -> Result<(u64, String), Box<dyn Error>> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut hash = Md5::new();
    let mut size = 0;
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((size, format!("{:x}", hash.finalize())))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: upload_verify BUCKET UNIQUE_KEY LOCAL_FILE".into());
    }
    let (bucket, key, path) = (&args[1], &args[2], Path::new(&args[3]));
    let client = Client::builder()
        .endpoint(std::env::var("FDS_ENDPOINT")?)
        .credentials(
            std::env::var("FDS_ACCESS_KEY_ID")?,
            std::env::var("FDS_ACCESS_KEY_SECRET")?,
        )
        .build()?;

    // 调用者必须保证文件在摘要计算及上传期间不变，并选择不会并发改写的 key。
    let (expected_size, expected_md5) = file_digest(path).await?;
    let mut metadata = UserMetadata::new();
    metadata.insert("content-md5", &expected_md5);
    client
        .put_object(bucket, key, UploadBody::from_file(path).await?)
        .content_type("application/octet-stream")
        .user_metadata(metadata)
        .send()
        .await?;

    // 从此处开始的失败不回滚 PUT，也不自动删除远端对象。
    let head = client
        .head_object(bucket, key)
        .send()
        .await?
        .ok_or("uploaded object was not found by HEAD")?;
    let size = head
        .attributes
        .content_length
        .ok_or("HEAD response has no valid Content-Length")?;
    if size != expected_size {
        return Err("uploaded object size differs from local file".into());
    }
    let mut output = client.get_object(bucket, key).send().await?;
    let mut hash = Md5::new();
    let mut received = 0;
    while let Some(chunk) = output.chunk().await? {
        received += chunk.len() as u64;
        hash.update(&chunk);
    }
    let actual_md5 = format!("{:x}", hash.finalize());
    if received != expected_size || actual_md5 != expected_md5 {
        return Err("downloaded content differs from local file".into());
    }
    // MD5 仅检测意外内容差异，不提供抗篡改认证。
    let url = client
        .presign_object(bucket, key)
        .method(Method::GET)
        .expires_in(Duration::from_secs(600))
        .build()?;
    eprintln!("上传及读取校验通过：{received} 字节");
    println!("{url}");
    Ok(())
}
