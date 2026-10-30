use std::env;
use std::fs;
use std::path::Path;

/// 把内置的 toolbox-static 打成 toolbox.tar.gz 放进 OUT_DIR，
/// 运行时用 include_bytes! 嵌入 exe —— 单文件便携版也能自带工具箱。
fn main() {
    build_static_archive();
    println!("cargo:rerun-if-changed=toolbox-static");
    tauri_build::build();
}

fn build_static_archive() {
    let out_dir = match env::var("OUT_DIR") {
        Ok(value) => value,
        Err(_) => return,
    };
    let target = Path::new(&out_dir).join("toolbox.tar.gz");

    let manifest_dir = match env::var("CARGO_MANIFEST_DIR") {
        Ok(value) => value,
        Err(_) => return,
    };
    let source = Path::new(&manifest_dir).join("toolbox-static");
    if !source.join("index.html").is_file() {
        // 保证 include_bytes! 的文件一定存在，运行时解包失败会走降级路径。
        let _ = fs::File::create(&target);
        println!("cargo:warning=toolbox-static 缺少 index.html，内置静态包为空");
        return;
    }

    let file = match fs::File::create(&target) {
        Ok(file) => file,
        Err(error) => {
            println!("cargo:warning=创建 toolbox.tar.gz 失败：{error}");
            return;
        }
    };
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::best());
    let mut builder = tar::Builder::new(encoder);
    if let Err(error) = builder.append_dir_all(".", &source) {
        println!("cargo:warning=打包 toolbox-static 失败：{error}");
        return;
    }
    let encoder = match builder.into_inner() {
        Ok(encoder) => encoder,
        Err(error) => {
            println!("cargo:warning=结束 tar 写入失败：{error}");
            return;
        }
    };
    if let Err(error) = encoder.finish() {
        println!("cargo:warning=结束 gzip 写入失败：{error}");
    }
}
