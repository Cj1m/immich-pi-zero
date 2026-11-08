//! One image at a time. Jpeg bytes live under `{data}/thumbs`. This is not video transcoding.

use std::path::Path;
use std::process::Stdio;

const THUMB_EDGE: u32 = 250;
const PREVIEW_EDGE: u32 = 1440;

pub struct ThumbFiles {
    /// Pixel size after the EXIF orientation is applied. This is the asset width and height.
    pub width: i64,
    pub height: i64,
    /// Pixel size of the file before orientation. The Original is not rotated.
    pub raw_width: i64,
    pub raw_height: i64,
}

/// Decode a still and write a thumbnail and a preview. `None` means the bytes are not a still we can read.
pub fn write_image_thumbs(
    data_dir: &Path,
    asset_id: &str,
    source: &Path,
    orientation: Option<u16>,
) -> Option<ThumbFiles> {
    let image = image::ImageReader::open(source)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let raw_width = i64::from(image.width());
    let raw_height = i64::from(image.height());
    let image = apply_orientation(image, orientation);
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
    Some(ThumbFiles {
        width,
        height,
        raw_width,
        raw_height,
    })
}

/// Same order as Immich's sharp map: rotate, then a vertical flip, then a horizontal flip.
fn apply_orientation(image: image::DynamicImage, orientation: Option<u16>) -> image::DynamicImage {
    match orientation {
        Some(2) => image.fliph(),
        Some(3) => image.rotate180(),
        Some(4) => image.rotate180().fliph(),
        Some(5) => image.rotate270().flipv(),
        Some(6) => image.rotate90(),
        Some(7) => image.rotate90().flipv(),
        Some(8) => image.rotate270(),
        _ => image,
    }
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
    let width = i64::from(image.width());
    let height = i64::from(image.height());
    Some(ThumbFiles {
        width,
        height,
        raw_width: width,
        raw_height: height,
    })
}

pub fn remove_thumbs(data_dir: &Path, asset_id: &str) {
    let _ = std::fs::remove_dir_all(data_dir.join("thumbs").join(asset_id));
}

pub(crate) enum ProfileJpeg {
    Wrote(std::path::PathBuf),
    Undecodable,
}

/// Jpeg thumbnail under `{data}/profile/{userId}`, same edge as an asset thumbnail.
pub(crate) fn write_profile_jpeg(
    data_dir: &Path,
    user_id: &str,
    source: &Path,
) -> std::io::Result<ProfileJpeg> {
    let image = match image::ImageReader::open(source) {
        Ok(reader) => match reader.with_guessed_format() {
            Ok(reader) => match reader.decode() {
                Ok(image) => image,
                Err(_) => return Ok(ProfileJpeg::Undecodable),
            },
            Err(_) => return Ok(ProfileJpeg::Undecodable),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProfileJpeg::Undecodable);
        }
        Err(error) => return Err(error),
    };
    let directory = data_dir.join("profile").join(user_id);
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{}.jpg", crate::db::new_id()));
    write_jpeg(&image.thumbnail(THUMB_EDGE, THUMB_EDGE), &path)
        .ok_or_else(|| std::io::Error::other("profile thumbnail"))?;
    Ok(ProfileJpeg::Wrote(path))
}
