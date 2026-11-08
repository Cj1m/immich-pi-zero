//! JPEG EXIF for the detail panel. City, state, and country stay empty: this server does not geocode.
//! A clock with no offset is stored as that clock in UTC. An offset keeps the clock as the local time
//! and stores the absolute instant in `dateTimeOriginal`.

use std::io::{BufRead, Seek};
use std::path::Path;

use chrono::{Datelike, Duration, NaiveDate, Timelike};
use exif::{DateTime, In, Reader, Tag, Value};

#[derive(Debug, Default)]
pub(crate) struct ExifMeta {
    pub parsed: bool,
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens_model: Option<String>,
    pub orientation: Option<u16>,
    pub date_time_original: Option<String>,
    pub local_date_time: Option<String>,
    pub time_zone: Option<String>,
    pub f_number: Option<f64>,
    pub focal_length: Option<f64>,
    pub iso: Option<i64>,
    pub exposure_time: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub description: String,
    pub rating: Option<i64>,
    pub pixel_width: Option<i64>,
    pub pixel_height: Option<i64>,
}

pub(crate) fn is_sidewards(orientation: Option<u16>) -> bool {
    matches!(orientation, Some(5..=8))
}

pub(crate) fn read_path(path: &Path) -> ExifMeta {
    let Ok(file) = std::fs::File::open(path) else {
        return ExifMeta::default();
    };
    read_reader(&mut std::io::BufReader::new(file))
}

fn read_reader<R: BufRead + Seek>(reader: &mut R) -> ExifMeta {
    let Ok(exif) = Reader::new().read_from_container(reader) else {
        return ExifMeta::default();
    };
    let mut meta = ExifMeta {
        parsed: true,
        description: String::new(),
        ..ExifMeta::default()
    };
    meta.make = text(&exif, Tag::Make);
    meta.model = text(&exif, Tag::Model);
    meta.lens_model = text(&exif, Tag::LensModel).filter(|value| lens_name_ok(value));
    meta.orientation = uint(&exif, Tag::Orientation).and_then(|value| u16::try_from(value).ok());
    meta.description = text(&exif, Tag::ImageDescription).unwrap_or_default();
    meta.f_number = rational(&exif, Tag::FNumber);
    meta.focal_length = rational(&exif, Tag::FocalLength);
    meta.iso = iso_value(&exif);
    meta.exposure_time =
        field(&exif, Tag::ExposureTime).and_then(|item| exposure_text(&item.value));
    meta.rating = rating_value(&exif);
    if let Some((width, height)) = pixel_size(&exif) {
        meta.pixel_width = Some(width);
        meta.pixel_height = Some(height);
    }
    if let Some(times) = photo_times(&exif) {
        meta.date_time_original = Some(times.original);
        meta.local_date_time = Some(times.local);
        meta.time_zone = times.zone;
    }
    apply_gps(&exif, &mut meta);
    meta
}

struct PhotoTimes {
    original: String,
    local: String,
    zone: Option<String>,
}

fn photo_times(exif: &exif::Exif) -> Option<PhotoTimes> {
    let candidates = [
        (
            Tag::DateTimeOriginal,
            Tag::SubSecTimeOriginal,
            Tag::OffsetTimeOriginal,
        ),
        (
            Tag::DateTimeDigitized,
            Tag::SubSecTimeDigitized,
            Tag::OffsetTimeDigitized,
        ),
        (Tag::DateTime, Tag::SubSecTime, Tag::OffsetTime),
    ];
    for (date_tag, subsec_tag, offset_tag) in candidates {
        let Some(bytes) = ascii_bytes(exif, date_tag) else {
            continue;
        };
        let Ok(mut parsed) = DateTime::from_ascii(&bytes) else {
            continue;
        };
        if let Some(subsec) = ascii_bytes(exif, subsec_tag) {
            let _ = parsed.parse_subsec(&subsec);
        }
        if let Some(offset) = ascii_bytes(exif, offset_tag) {
            let _ = parsed.parse_offset(&offset);
        } else {
            apply_suffix_offset(&mut parsed, &bytes);
        }
        if let Some(times) = times_from_parts(&parsed) {
            return Some(times);
        }
    }
    gps_times(exif)
}

fn times_from_parts(parsed: &DateTime) -> Option<PhotoTimes> {
    let wall = NaiveDate::from_ymd_opt(
        i32::from(parsed.year),
        u32::from(parsed.month),
        u32::from(parsed.day),
    )?
    .and_hms_opt(
        u32::from(parsed.hour),
        u32::from(parsed.minute),
        u32::from(parsed.second),
    )?;
    let millis = parsed.nanosecond.unwrap_or(0) / 1_000_000;
    let local = format_utc(&wall, millis);
    let (original, zone) = if let Some(minutes) = parsed.offset {
        let instant = wall.checked_sub_signed(Duration::minutes(i64::from(minutes)))?;
        (format_utc(&instant, millis), Some(format_zone(minutes)))
    } else {
        (local.clone(), None)
    };
    Some(PhotoTimes {
        original,
        local,
        zone,
    })
}

fn apply_suffix_offset(parsed: &mut DateTime, bytes: &[u8]) {
    if bytes.len() <= 19 {
        return;
    }
    let rest = bytes[19..].trim_ascii();
    if rest.first() == Some(&b'Z') {
        parsed.offset = Some(0);
    } else if rest.len() >= 6 && (rest[0] == b'+' || rest[0] == b'-') {
        let _ = parsed.parse_offset(rest);
    }
}

fn gps_times(exif: &exif::Exif) -> Option<PhotoTimes> {
    let date = String::from_utf8(ascii_bytes(exif, Tag::GPSDateStamp)?).ok()?;
    let time = field(exif, Tag::GPSTimeStamp)?;
    let Value::Rational(parts) = &time.value else {
        return None;
    };
    if parts.len() < 3 {
        return None;
    }
    let hour = rational_f64(&parts[0])?;
    let minute = rational_f64(&parts[1])?;
    let second = rational_f64(&parts[2])?;
    if !(hour.is_finite() && minute.is_finite() && second.is_finite()) {
        return None;
    }
    let whole = second.floor() as u32;
    let millis = ((second - f64::from(whole)) * 1000.0).round() as u32;
    let text = format!(
        "{date} {:02}:{:02}:{:02}",
        hour.round() as u32,
        minute.round() as u32,
        whole
    );
    let mut parsed = DateTime::from_ascii(text.as_bytes()).ok()?;
    parsed.nanosecond = Some(millis.saturating_mul(1_000_000));
    parsed.offset = Some(0);
    times_from_parts(&parsed)
}

fn format_utc(wall: &chrono::NaiveDateTime, millis: u32) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        wall.year(),
        wall.month(),
        wall.day(),
        wall.hour(),
        wall.minute(),
        wall.second(),
        millis
    )
}

fn format_zone(minutes: i16) -> String {
    if minutes == 0 {
        return "UTC+0".to_string();
    }
    let sign = if minutes < 0 { '-' } else { '+' };
    let abs = minutes.unsigned_abs();
    format!("{sign}{:02}:{:02}", abs / 60, abs % 60)
}

fn apply_gps(exif: &exif::Exif, meta: &mut ExifMeta) {
    let latitude = gps_coordinate(
        exif,
        Tag::GPSLatitude,
        text(exif, Tag::GPSLatitudeRef).as_deref(),
    );
    let longitude = gps_coordinate(
        exif,
        Tag::GPSLongitude,
        text(exif, Tag::GPSLongitudeRef).as_deref(),
    );
    if let (Some(latitude), Some(longitude)) = (latitude, longitude) {
        if latitude.is_finite() && longitude.is_finite() && (latitude != 0.0 || longitude != 0.0) {
            meta.latitude = Some(latitude);
            meta.longitude = Some(longitude);
        }
    }
}

fn gps_coordinate(exif: &exif::Exif, tag: Tag, reference: Option<&str>) -> Option<f64> {
    let field = field(exif, tag)?;
    let Value::Rational(parts) = &field.value else {
        return None;
    };
    let value = if parts.len() >= 3 {
        rational_f64(&parts[0])?
            + rational_f64(&parts[1])? / 60.0
            + rational_f64(&parts[2])? / 3600.0
    } else if parts.len() == 1 {
        rational_f64(&parts[0])?
    } else {
        return None;
    };
    if !value.is_finite() {
        return None;
    }
    let negative = matches!(reference, Some("S") | Some("W"));
    Some(if negative { -value.abs() } else { value.abs() })
}

fn pixel_size(exif: &exif::Exif) -> Option<(i64, i64)> {
    let width = uint(exif, Tag::ImageWidth).or_else(|| uint(exif, Tag::PixelXDimension))?;
    let height = uint(exif, Tag::ImageLength).or_else(|| uint(exif, Tag::PixelYDimension))?;
    if width == 0 || height == 0 {
        return None;
    }
    Some((i64::from(width), i64::from(height)))
}

fn iso_value(exif: &exif::Exif) -> Option<i64> {
    let value = uint(exif, Tag::RecommendedExposureIndex)
        .or_else(|| uint(exif, Tag::StandardOutputSensitivity))
        .or_else(|| uint(exif, Tag::PhotographicSensitivity))?;
    Some(i64::from(value))
}

fn rating_value(exif: &exif::Exif) -> Option<i64> {
    let tag = Tag(exif::Context::Tiff, 0x4746);
    let value = i64::from(uint(exif, tag)?);
    if (1..=5).contains(&value) {
        Some(value)
    } else {
        None
    }
}

fn lens_name_ok(value: &str) -> bool {
    !value.is_empty() && value != "----" && !value.starts_with("Unknown")
}

fn field(exif: &exif::Exif, tag: Tag) -> Option<&exif::Field> {
    exif.get_field(tag, In::PRIMARY)
}

fn text(exif: &exif::Exif, tag: Tag) -> Option<String> {
    let bytes = ascii_bytes(exif, tag)?;
    let text = String::from_utf8_lossy(&bytes)
        .trim()
        .trim_matches('\0')
        .to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn ascii_bytes(exif: &exif::Exif, tag: Tag) -> Option<Vec<u8>> {
    match &field(exif, tag)?.value {
        Value::Ascii(parts) => parts.first().cloned(),
        _ => None,
    }
}

fn uint(exif: &exif::Exif, tag: Tag) -> Option<u32> {
    field(exif, tag)?.value.get_uint(0)
}

fn rational(exif: &exif::Exif, tag: Tag) -> Option<f64> {
    let field = field(exif, tag)?;
    let Value::Rational(parts) = &field.value else {
        return None;
    };
    rational_f64(parts.first()?)
}

fn rational_f64(rational: &exif::Rational) -> Option<f64> {
    if rational.denom == 0 {
        None
    } else {
        let number = rational.num as f64 / rational.denom as f64;
        number.is_finite().then_some(number)
    }
}

fn exposure_text(value: &Value) -> Option<String> {
    let Value::Rational(parts) = value else {
        return None;
    };
    let rational = parts.first()?;
    if rational.denom == 0 || rational.num == 0 {
        return None;
    }
    if rational.num >= rational.denom {
        return Some(trim_float(rational.num as f64 / rational.denom as f64));
    }
    let denom = rational.denom as f64 / rational.num as f64;
    if (denom - denom.round()).abs() < 1e-6 {
        Some(format!("1/{}", denom.round() as u64))
    } else {
        Some(format!("1/{denom}"))
    }
}

fn trim_float(number: f64) -> String {
    if (number - number.round()).abs() < 1e-6 {
        format!("{}", number.round() as i64)
    } else {
        let text = format!("{number:.4}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Fields written into a JPEG APP1 segment by tests.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct ExifFixture {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens_model: Option<String>,
    pub description: Option<String>,
    pub orientation: Option<u16>,
    pub rating: Option<u16>,
    pub date_time_original: Option<String>,
    pub offset_time_original: Option<String>,
    pub subsec_time_original: Option<String>,
    pub iso: Option<u16>,
    pub exposure: Option<(u32, u32)>,
    pub f_number: Option<(u32, u32)>,
    pub focal_length: Option<(u32, u32)>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

/// Insert an Exif APP1 segment after the JPEG start marker.
#[doc(hidden)]
pub fn embed_exif(jpeg: &[u8], fixture: &ExifFixture) -> Vec<u8> {
    let Some(tiff) = fixture_tiff(fixture) else {
        return jpeg.to_vec();
    };
    inject_app1(jpeg, &tiff)
}

fn inject_app1(jpeg: &[u8], tiff: &[u8]) -> Vec<u8> {
    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return jpeg.to_vec();
    }
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(tiff);
    let len = payload.len() + 2;
    if len > u16::MAX as usize {
        return jpeg.to_vec();
    }
    let mut out = Vec::with_capacity(jpeg.len() + len + 2);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

struct TiffEntry {
    tag: u16,
    typ: u16,
    count: u32,
    data: Vec<u8>,
}

fn fixture_tiff(fixture: &ExifFixture) -> Option<Vec<u8>> {
    let mut exif_entries = Vec::new();
    if let Some(date) = &fixture.date_time_original {
        exif_entries.push(ascii_entry(0x9003, date));
    }
    if let Some(offset) = &fixture.offset_time_original {
        exif_entries.push(ascii_entry(0x9011, offset));
    }
    if let Some(subsec) = &fixture.subsec_time_original {
        exif_entries.push(ascii_entry(0x9291, subsec));
    }
    if let Some(iso) = fixture.iso {
        exif_entries.push(short_entry(0x8827, iso));
    }
    if let Some((num, denom)) = fixture.exposure {
        exif_entries.push(rational_entry(0x829A, &[(num, denom)]));
    }
    if let Some((num, denom)) = fixture.f_number {
        exif_entries.push(rational_entry(0x829D, &[(num, denom)]));
    }
    if let Some((num, denom)) = fixture.focal_length {
        exif_entries.push(rational_entry(0x920A, &[(num, denom)]));
    }
    if let Some(lens) = &fixture.lens_model {
        exif_entries.push(ascii_entry(0xA434, lens));
    }

    let mut gps_entries = Vec::new();
    if let (Some(latitude), Some(longitude)) = (fixture.latitude, fixture.longitude) {
        let (lat_parts, lat_south) = decimal_dms(latitude);
        let (lon_parts, lon_west) = decimal_dms(longitude);
        gps_entries.push(ascii_entry(0x0001, if lat_south { "S" } else { "N" }));
        gps_entries.push(rational_entry(0x0002, &lat_parts));
        gps_entries.push(ascii_entry(0x0003, if lon_west { "W" } else { "E" }));
        gps_entries.push(rational_entry(0x0004, &lon_parts));
    }

    let mut parent = Vec::new();
    if let Some(description) = &fixture.description {
        parent.push(ascii_entry(0x010E, description));
    }
    if let Some(make) = &fixture.make {
        parent.push(ascii_entry(0x010F, make));
    }
    if let Some(model) = &fixture.model {
        parent.push(ascii_entry(0x0110, model));
    }
    if let Some(orientation) = fixture.orientation {
        parent.push(short_entry(0x0112, orientation));
    }
    if let Some(rating) = fixture.rating {
        parent.push(short_entry(0x4746, rating));
    }
    if parent.is_empty() && exif_entries.is_empty() && gps_entries.is_empty() {
        return None;
    }
    if !exif_entries.is_empty() {
        parent.push(long_entry(0x8769, 0));
    }
    if !gps_entries.is_empty() {
        parent.push(long_entry(0x8825, 0));
    }

    let parent_at = 8usize;
    let exif_at = parent_at + ifd_span(&parent);
    let gps_at = exif_at
        + if !exif_entries.is_empty() {
            ifd_span(&exif_entries)
        } else {
            0
        };
    if !exif_entries.is_empty() {
        set_long(&mut parent, 0x8769, exif_at as u32);
    }
    if !gps_entries.is_empty() {
        set_long(&mut parent, 0x8825, gps_at as u32);
    }

    let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
    tiff.extend(write_ifd(&mut parent, parent_at));
    if !exif_entries.is_empty() {
        tiff.extend(write_ifd(&mut exif_entries, exif_at));
    }
    if !gps_entries.is_empty() {
        tiff.extend(write_ifd(&mut gps_entries, gps_at));
    }
    Some(tiff)
}

fn decimal_dms(value: f64) -> (Vec<(u32, u32)>, bool) {
    let negative = value < 0.0;
    let micro = (value.abs() * 1_000_000.0).round() as u64;
    let degrees = (micro / 1_000_000) as u32;
    let rem = micro % 1_000_000;
    let minutes = ((rem * 60) / 1_000_000) as u32;
    let rem2 = (rem * 60) % 1_000_000;
    let seconds_thousandths = ((rem2 * 60) / 1_000) as u32;
    (
        vec![(degrees, 1), (minutes, 1), (seconds_thousandths, 1000)],
        negative,
    )
}

fn ascii_entry(tag: u16, text: &str) -> TiffEntry {
    let mut data = text.as_bytes().to_vec();
    data.push(0);
    TiffEntry {
        tag,
        typ: 2,
        count: data.len() as u32,
        data,
    }
}

fn short_entry(tag: u16, value: u16) -> TiffEntry {
    TiffEntry {
        tag,
        typ: 3,
        count: 1,
        data: value.to_le_bytes().to_vec(),
    }
}

fn long_entry(tag: u16, value: u32) -> TiffEntry {
    TiffEntry {
        tag,
        typ: 4,
        count: 1,
        data: value.to_le_bytes().to_vec(),
    }
}

fn rational_entry(tag: u16, parts: &[(u32, u32)]) -> TiffEntry {
    let mut data = Vec::with_capacity(parts.len() * 8);
    for (num, denom) in parts {
        data.extend(num.to_le_bytes());
        data.extend(denom.to_le_bytes());
    }
    TiffEntry {
        tag,
        typ: 5,
        count: parts.len() as u32,
        data,
    }
}

fn set_long(entries: &mut [TiffEntry], tag: u16, value: u32) {
    if let Some(entry) = entries.iter_mut().find(|entry| entry.tag == tag) {
        entry.data = value.to_le_bytes().to_vec();
    }
}

fn ifd_span(entries: &[TiffEntry]) -> usize {
    let data: usize = entries
        .iter()
        .filter(|entry| entry.data.len() > 4)
        .map(|entry| entry.data.len())
        .sum();
    2 + 12 * entries.len() + 4 + data
}

fn write_ifd(entries: &mut [TiffEntry], at: usize) -> Vec<u8> {
    entries.sort_by_key(|entry| entry.tag);
    let ifd_len = 2 + 12 * entries.len() + 4;
    let mut data = Vec::new();
    let mut ifd = Vec::with_capacity(ifd_len);
    ifd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for entry in entries.iter() {
        ifd.extend_from_slice(&entry.tag.to_le_bytes());
        ifd.extend_from_slice(&entry.typ.to_le_bytes());
        ifd.extend_from_slice(&entry.count.to_le_bytes());
        if entry.data.len() <= 4 {
            let mut slot = [0u8; 4];
            slot[..entry.data.len()].copy_from_slice(&entry.data);
            ifd.extend_from_slice(&slot);
        } else {
            let offset = (at + ifd_len + data.len()) as u32;
            ifd.extend_from_slice(&offset.to_le_bytes());
            data.extend_from_slice(&entry.data);
        }
    }
    ifd.extend_from_slice(&0u32.to_le_bytes());
    ifd.extend_from_slice(&data);
    ifd
}

#[cfg(test)]
mod tests {
    use super::{embed_exif, read_reader, ExifFixture};

    fn tiny_jpeg() -> Vec<u8> {
        let image = image::RgbImage::from_pixel(4, 2, image::Rgb([9, 8, 7]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        bytes
    }

    fn read_fixture(fixture: &ExifFixture) -> super::ExifMeta {
        let bytes = embed_exif(&tiny_jpeg(), fixture);
        read_reader(&mut std::io::Cursor::new(bytes))
    }

    #[test]
    fn no_offset_keeps_the_clock_in_both_datetimes() {
        let meta = read_fixture(&ExifFixture {
            date_time_original: Some("2020:05:04 03:02:01".to_string()),
            subsec_time_original: Some("12".to_string()),
            make: Some("Canon".to_string()),
            ..ExifFixture::default()
        });
        assert_eq!(meta.make.as_deref(), Some("Canon"));
        assert_eq!(
            meta.date_time_original.as_deref(),
            Some("2020-05-04T03:02:01.120Z")
        );
        assert_eq!(
            meta.local_date_time.as_deref(),
            Some("2020-05-04T03:02:01.120Z")
        );
        assert!(meta.time_zone.is_none());
        assert!(meta.latitude.is_none());
    }

    #[test]
    fn offset_splits_the_local_clock_from_the_utc_instant() {
        let meta = read_fixture(&ExifFixture {
            date_time_original: Some("2020:05:04 01:00:00".to_string()),
            offset_time_original: Some("+02:00".to_string()),
            ..ExifFixture::default()
        });
        assert_eq!(
            meta.local_date_time.as_deref(),
            Some("2020-05-04T01:00:00.000Z")
        );
        assert_eq!(
            meta.date_time_original.as_deref(),
            Some("2020-05-03T23:00:00.000Z")
        );
        assert_eq!(meta.time_zone.as_deref(), Some("+02:00"));
    }

    #[test]
    fn zero_gps_is_not_a_place_and_south_west_are_negative() {
        let zero = read_fixture(&ExifFixture {
            latitude: Some(0.0),
            longitude: Some(0.0),
            ..ExifFixture::default()
        });
        assert!(zero.parsed);
        assert!(zero.latitude.is_none());
        assert!(zero.longitude.is_none());

        let south = read_fixture(&ExifFixture {
            latitude: Some(-33.5),
            longitude: Some(18.25),
            ..ExifFixture::default()
        });
        let latitude = south.latitude.unwrap();
        let longitude = south.longitude.unwrap();
        assert!((latitude + 33.5).abs() < 0.000_1, "{latitude}");
        assert!((longitude - 18.25).abs() < 0.000_1, "{longitude}");
    }

    #[test]
    fn a_plain_jpeg_has_no_exif() {
        let meta = read_reader(&mut std::io::Cursor::new(tiny_jpeg()));
        assert!(!meta.parsed);
        assert!(meta.date_time_original.is_none());
        assert!(meta.make.is_none());
    }
}
