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

pub(crate) enum StillEdit {
    Crop { x: u32, y: u32, width: u32, height: u32 },
    Rotate(u32),
    MirrorHorizontal,
    MirrorVertical,
}

pub(crate) fn oriented_size(source: &Path, orientation: Option<u16>) -> Option<(i64, i64)> {
    let image = decode_oriented(source, orientation)?;
    Some((i64::from(image.width()), i64::from(image.height())))
}

pub(crate) fn remove_edited_stills(data_dir: &Path, asset_id: &str) {
    let directory = data_dir.join("thumbs").join(asset_id);
    let _ = std::fs::remove_file(directory.join("thumbnail_edited.jpg"));
    let _ = std::fs::remove_file(directory.join("preview_edited.jpg"));
}

/// Orientation first, then each edit. The size is the full edited image, before the preview shrinks.
pub(crate) fn write_edited_stills(
    data_dir: &Path,
    asset_id: &str,
    source: &Path,
    orientation: Option<u16>,
    steps: &[StillEdit],
) -> Option<(i64, i64)> {
    let mut image = decode_oriented(source, orientation)?;
    for step in steps {
        image = apply_still(image, step)?;
    }
    let width = i64::from(image.width());
    let height = i64::from(image.height());
    let directory = data_dir.join("thumbs").join(asset_id);
    std::fs::create_dir_all(&directory).ok()?;
    let thumbnail = if image.width() <= THUMB_EDGE && image.height() <= THUMB_EDGE {
        image.clone()
    } else {
        image.thumbnail(THUMB_EDGE, THUMB_EDGE)
    };
    write_jpeg(&thumbnail, &directory.join("thumbnail_edited.jpg"))?;
    let preview = if image.width() <= PREVIEW_EDGE && image.height() <= PREVIEW_EDGE {
        image
    } else {
        image.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE)
    };
    write_jpeg(&preview, &directory.join("preview_edited.jpg"))?;
    Some((width, height))
}

fn decode_oriented(source: &Path, orientation: Option<u16>) -> Option<image::DynamicImage> {
    let image = image::ImageReader::open(source)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    Some(apply_orientation(image, orientation))
}

fn apply_still(image: image::DynamicImage, step: &StillEdit) -> Option<image::DynamicImage> {
    match step {
        StillEdit::Crop { x, y, width, height } => {
            if x.saturating_add(*width) > image.width() || y.saturating_add(*height) > image.height()
            {
                return None;
            }
            Some(image.crop_imm(*x, *y, *width, *height))
        }
        StillEdit::Rotate(0) => Some(image),
        StillEdit::Rotate(90) => Some(image.rotate90()),
        StillEdit::Rotate(180) => Some(image.rotate180()),
        StillEdit::Rotate(270) => Some(image.rotate270()),
        StillEdit::Rotate(_) => None,
        StillEdit::MirrorHorizontal => Some(image.flipv()),
        StillEdit::MirrorVertical => Some(image.fliph()),
    }
}

pub(crate) struct FaceCrop {
    pub width: i64,
    pub height: i64,
    pub x1: i64,
    pub y1: i64,
    pub x2: i64,
    pub y2: i64,
}

/// The file `PersonGenerateThumbnail` would write: a 250-edge JPEG cropped from the featured face.
/// There is no job queue, so the request writes it before it returns.
pub(crate) fn write_person_jpeg(
    data_dir: &Path,
    owner_id: &str,
    person_id: &str,
    source: &Path,
    crop: FaceCrop,
) -> Option<std::path::PathBuf> {
    if person_id.len() < 4 || crop.width <= 0 || crop.height <= 0 {
        return None;
    }
    let image = image::ImageReader::open(source)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let (x, y, edge) = face_square(&image, &crop)?;
    let face = image
        .crop_imm(x, y, edge, edge)
        .thumbnail(THUMB_EDGE, THUMB_EDGE);
    let (prefix, rest) = person_id.split_at(2);
    let (next, _) = rest.split_at(2);
    let directory = data_dir
        .join("thumbs")
        .join(owner_id)
        .join(prefix)
        .join(next);
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join(format!("{person_id}.jpeg"));
    write_jpeg(&face, &path)?;
    Some(path)
}

/// Same square as v3.3.6 `getCrop`: clamp the box, scale it onto the decoded image, then zoom out 10%.
fn face_square(image: &image::DynamicImage, crop: &FaceCrop) -> Option<(u32, u32, u32)> {
    let old_w = crop.width as f64;
    let old_h = crop.height as f64;
    let new_w = f64::from(image.width());
    let new_h = f64::from(image.height());
    if new_w < 2.0 || new_h < 2.0 {
        return None;
    }
    let x1 = (crop.x1.clamp(0, crop.width) as f64).min(old_w);
    let y1 = (crop.y1.clamp(0, crop.height) as f64).min(old_h);
    let x2 = (crop.x2.clamp(0, crop.width) as f64).min(old_w);
    let y2 = (crop.y2.clamp(0, crop.height) as f64).min(old_h);
    let width_scale = new_w / old_w;
    let height_scale = new_h / old_h;
    let half_w = width_scale * (x2 - x1) / 2.0;
    let half_h = height_scale * (y2 - y1) / 2.0;
    if half_w <= 0.0 && half_h <= 0.0 {
        return None;
    }
    let middle_x = (width_scale * x1 + half_w).round();
    let middle_y = (height_scale * y1 + half_h).round();
    let target = (half_w.max(half_h) * 1.1).floor();
    let new_half = [
        middle_x - (middle_x - target).max(0.0),
        middle_y - (middle_y - target).max(0.0),
        (new_w - 1.0).min(middle_x + target) - middle_x,
        (new_h - 1.0).min(middle_y + target) - middle_y,
    ]
    .into_iter()
    .fold(f64::INFINITY, f64::min);
    if new_half < 1.0 {
        return None;
    }
    let mut x = (middle_x - new_half).round().max(0.0) as u32;
    let mut y = (middle_y - new_half).round().max(0.0) as u32;
    let mut edge = (new_half * 2.0).round().max(1.0) as u32;
    if x >= image.width() || y >= image.height() {
        return None;
    }
    edge = edge.min(image.width() - x).min(image.height() - y);
    if edge == 0 {
        return None;
    }
    x = x.min(image.width() - edge);
    y = y.min(image.height() - edge);
    Some((x, y, edge))
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
