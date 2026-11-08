//! One image at a time. Jpeg bytes live under `{data}/thumbs`. This is not video transcoding.

use std::path::Path;
use std::process::Stdio;

const THUMB_EDGE: u32 = 250;
const PREVIEW_EDGE: u32 = 1440;

pub struct ThumbFiles {
    pub width: i64,
    pub height: i64,
}

/// Decode a still and write a thumbnail and a preview. `None` means the bytes are not a still we can read.
pub fn write_image_thumbs(data_dir: &Path, asset_id: &str, source: &Path) -> Option<ThumbFiles> {
    let image = image::ImageReader::open(source)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let width = i64::from(image.width());
    let height = i64::from(image.height());
    let directory = data_dir.join("thumbs").join(asset_id);
    std::fs::create_dir_all(&directory).ok()?;
    let thumbnail = directory.join("thumbnail.jpg");
    let preview = directory.join("preview.jpg");
    write_jpeg(&image.thumbnail(THUMB_EDGE, THUMB_EDGE), &thumbnail)?;
    let preview_image = if image.width() <= PREVIEW_EDGE && image.height() <= PREVIEW_EDGE {
        image
    } else {
        image.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE)
    };
    write_jpeg(&preview_image, &preview)?;
    Some(ThumbFiles { width, height })
}

fn write_jpeg(image: &image::DynamicImage, path: &Path) -> Option<()> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Jpeg).ok()?;
    std::fs::write(path, bytes.into_inner()).ok()
}

/// One frame, saved as a jpeg thumbnail. No encoded video is written.
pub fn write_video_frame(data_dir: &Path, asset_id: &str, source: &Path) -> Option<ThumbFiles> {
    let directory = data_dir.join("thumbs").join(asset_id);
    std::fs::create_dir_all(&directory).ok()?;
    let preview = directory.join("preview.jpg");
    let status = std::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(source)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg("scale='min(1440,iw)':-2")
        .arg(&preview)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if !status.success() || !preview.exists() {
        return None;
    }
    let image = image::ImageReader::open(&preview)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let thumbnail = directory.join("thumbnail.jpg");
    write_jpeg(&image.thumbnail(THUMB_EDGE, THUMB_EDGE), &thumbnail)?;
    Some(ThumbFiles {
        width: i64::from(image.width()),
        height: i64::from(image.height()),
    })
}

pub fn remove_thumbs(data_dir: &Path, asset_id: &str) {
    let _ = std::fs::remove_dir_all(data_dir.join("thumbs").join(asset_id));
}
