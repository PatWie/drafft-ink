#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Pictures through `picture::prepare`: which formats are read, what is stored for them, and what is refused.
//!
//! The fixtures are small and built here: bytes written by hand (so a mistake shared by an encoder and its decoder
//! cannot hide), or by the `image` crate's encoders where the format is too long to write by hand.

use std::io::Cursor;

use drafftink_io::picture::{self, Kind, PictureError};
use image::{ImageFormat, Rgba, RgbaImage};

const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

fn pixels(stored: &picture::Prepared) -> RgbaImage {
    assert_eq!(stored.mime, "image/png", "GIF, BMP and ICO are stored as PNG");
    image::load_from_memory_with_format(&stored.bytes, ImageFormat::Png).unwrap().to_rgba8()
}

fn png_of(img: &RgbaImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

// ---- GIF ----

/// A 1x1 GIF of one pixel, written by hand: a table of black and white, then the picture. The byte at
/// `GIF_TRANSPARENT` is the flags of the graphic control block, where bit 0 makes colour 0 transparent.
const GIF_1X1: &[u8] = &[
    b'G', b'I', b'F', b'8', b'9', b'a', 1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0x21, 0xf9, 4, 0, 0, 0, 0,
    0, 0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, 0x3b,
];
const GIF_TRANSPARENT: usize = 22;

/// A GIF with two frames: the first red, the second blue.
fn gif_two_frames() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
        for color in [RED, BLUE] {
            enc.encode_frame(image::Frame::new(RgbaImage::from_pixel(5, 3, color))).unwrap();
        }
    }
    out
}

#[test]
fn a_gif_is_stored_as_a_png_of_its_first_frame() {
    let ready = picture::prepare(gif_two_frames(), true).unwrap();
    assert_eq!((ready.given, ready.natural, ready.shrunk), ((5, 3), (5, 3), false));
    assert!(pixels(&ready).pixels().all(|p| *p == RED), "the first frame is red, the second blue");
}

#[test]
fn a_hand_written_gif_is_read() {
    assert_eq!(picture::sniff(GIF_1X1), Some(Kind::Gif));
    let ready = picture::prepare(GIF_1X1.to_vec(), true).unwrap();
    assert_eq!(ready.natural, (1, 1));
    assert_eq!(pixels(&ready).get_pixel(0, 0).0, [0, 0, 0, 255], "colour 0 of the table is black");
    // The same picture with its transparent colour switched on keeps the transparency.
    let mut clear = GIF_1X1.to_vec();
    clear[GIF_TRANSPARENT] = 1;
    assert_eq!(pixels(&picture::prepare(clear, true).unwrap()).get_pixel(0, 0).0[3], 0);
}

#[test]
fn a_gif_that_claims_a_huge_screen_is_refused_before_it_is_decoded() {
    let mut huge = GIF_1X1.to_vec();
    huge[6..8].copy_from_slice(&60_000u16.to_le_bytes());
    huge[8..10].copy_from_slice(&60_000u16.to_le_bytes());
    assert!(matches!(picture::prepare(huge, true), Err(PictureError::TooManyPixels { .. })));
}

// ---- BMP ----

/// A 24-bit BMP written by hand: `rows` from the top, each a list of `[r, g, b]`.
fn bmp(rows: &[&[[u8; 3]]]) -> Vec<u8> {
    let (w, h) = (rows[0].len(), rows.len());
    let row_bytes = (w * 3).div_ceil(4) * 4;
    let size = 54 + row_bytes * h;
    let mut out = b"BM".to_vec();
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as u32).to_le_bytes());
    out.extend_from_slice(&(h as u32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    // The rows are stored bottom to top, the colours as blue, green, red.
    for row in rows.iter().rev() {
        let mut bytes: Vec<u8> = row.iter().flat_map(|[r, g, b]| [*b, *g, *r]).collect();
        bytes.resize(row_bytes, 0);
        out.extend_from_slice(&bytes);
    }
    out
}

#[test]
fn a_bmp_is_stored_as_a_png() {
    let red = [255, 0, 0];
    let blue = [0, 0, 255];
    let ready = picture::prepare(bmp(&[&[red, blue, red], &[blue, red, blue]]), true).unwrap();
    assert_eq!((ready.given, ready.natural), ((3, 2), (3, 2)));
    let img = pixels(&ready);
    assert_eq!((img.get_pixel(0, 0), img.get_pixel(1, 0), img.get_pixel(0, 1)), (&RED, &BLUE, &BLUE), "top row first");
}

#[test]
fn bmp_needs_more_than_two_letters() {
    // Text that starts with "BM" is not a picture, and a real BMP is known by its header size too.
    assert_eq!(picture::sniff(b"BMW is a car maker, and this is its story"), None);
    assert_eq!(picture::sniff(&bmp(&[&[[1, 2, 3]]])), Some(Kind::Bmp));
}

#[test]
fn a_bmp_that_claims_a_huge_size_is_refused() {
    let mut huge = bmp(&[&[[1, 2, 3]]]);
    huge[18..22].copy_from_slice(&60_000u32.to_le_bytes());
    huge[22..26].copy_from_slice(&60_000u32.to_le_bytes());
    assert!(matches!(picture::prepare(huge, true), Err(PictureError::TooManyPixels { .. })));
}

// ---- ICO ----

/// An icon file of PNG images. Each entry is `(side, declared bits per pixel, color)`. An entry's data is a PNG of
/// that colour, `side` pixels wide.
fn ico(entries: &[(u32, u16, Rgba<u8>)]) -> Vec<u8> {
    let images: Vec<Vec<u8>> =
        entries.iter().map(|(side, _, color)| png_of(&RgbaImage::from_pixel(*side, *side, *color))).collect();
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * entries.len();
    for ((side, bits, _), data) in entries.iter().zip(&images) {
        // A side of 256 is written as 0.
        let byte = u8::try_from(*side).unwrap_or(0);
        out.extend_from_slice(&[byte, byte, 0, 0, 1, 0]);
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    images.iter().for_each(|data| out.extend_from_slice(data));
    out
}

#[test]
fn an_ico_is_stored_as_its_largest_image() {
    // The small image has the deeper colour, which the decoder would prefer. The largest one is wanted.
    let file = ico(&[(16, 32, RED), (48, 8, BLUE), (32, 32, RED)]);
    assert_eq!(picture::sniff(&file), Some(Kind::Ico));
    let ready = picture::prepare(file, true).unwrap();
    assert_eq!((ready.given, ready.natural), ((48, 48), (48, 48)));
    assert!(pixels(&ready).pixels().all(|p| *p == BLUE));
}

#[test]
fn an_ico_with_one_image_and_one_of_256_pixels() {
    let ready = picture::prepare(ico(&[(256, 32, RED)]), true).unwrap();
    assert_eq!(ready.natural, (256, 256));
    let ready = picture::prepare(ico(&[(8, 32, RED)]), true).unwrap();
    assert_eq!(ready.natural, (8, 8));
}

#[test]
fn a_damaged_ico_is_refused_without_panic() {
    let good = ico(&[(16, 32, RED), (32, 32, BLUE)]);
    // The directory promises more entries than the file holds.
    let mut many = good.clone();
    many[4..6].copy_from_slice(&500u16.to_le_bytes());
    assert!(picture::prepare(many, true).is_err());
    // An entry whose data lies outside the file.
    let mut outside = good.clone();
    outside[6 + 16 + 12..6 + 16 + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(picture::prepare(outside, true), Err(PictureError::Unreadable(_))));
    // Cut short in the middle of the image data.
    assert!(picture::prepare(good[..good.len() - 40].to_vec(), true).is_err());
    // No entries at all is not an icon.
    assert!(matches!(picture::prepare(vec![0, 0, 1, 0, 0, 0], true), Err(PictureError::Unsupported { .. })));
}

// ---- all of them ----

#[test]
fn damaged_and_cut_pictures_of_every_format_never_panic() {
    let samples = [
        GIF_1X1.to_vec(),
        gif_two_frames(),
        bmp(&[&[[255, 0, 0], [0, 0, 255]], &[[0, 0, 255], [255, 0, 0]]]),
        ico(&[(16, 32, RED), (24, 32, BLUE)]),
    ];
    for good in &samples {
        for i in 0..good.len() {
            let mut flipped = good.clone();
            flipped[i] ^= 0xff;
            let _ = picture::prepare(flipped, true);
            let _ = picture::prepare(good[..i].to_vec(), true);
        }
    }
}

// ---- SVG ----

const SVG_NS: &str = "xmlns=\"http://www.w3.org/2000/svg\"";

fn svg(attrs: &str, body: &str) -> Vec<u8> {
    format!("<svg {SVG_NS} {attrs}>{body}</svg>").into_bytes()
}

/// The error text of a refused picture.
fn why(result: Result<picture::Prepared, PictureError>) -> String {
    match result {
        Err(e) => e.to_string(),
        Ok(p) => panic!("the picture was accepted: given {:?}, natural {:?}", p.given, p.natural),
    }
}

#[test]
fn an_svg_is_drawn_at_twice_its_size_and_shown_at_its_own() {
    let file = svg("width=\"40\" height=\"20\"", "<rect width=\"40\" height=\"20\" fill=\"#ff0000\"/>");
    assert_eq!(picture::sniff(&file), Some(Kind::Svg));
    let ready = picture::prepare(file, true).unwrap();
    // The long side is 40, so 80 would be drawn; the stored bitmap is at least 512.
    assert_eq!((ready.given, ready.natural, ready.shrunk), ((40, 20), (512, 256), false));
    assert!(pixels(&ready).pixels().all(|p| *p == RED));
}

#[test]
fn the_size_comes_from_the_view_box_or_the_units() {
    let size = |attrs: &str| picture::prepare(svg(attrs, ""), true).unwrap().given;
    assert_eq!(size("viewBox=\"0 0 100 50\""), (100, 50), "no width and height: the view box");
    assert_eq!(size("width=\"1in\" height=\"0.5in\""), (96, 48), "units");
    assert_eq!(size("width=\"200\" height=\"100\" viewBox=\"0 0 10 10\""), (200, 100), "width and height win");
    let big = picture::prepare(svg("width=\"4000\" height=\"1000\"", ""), true).unwrap();
    assert_eq!((big.given, big.natural), ((4000, 1000), (2048, 512)), "at most 2048 on the long side");
}

#[test]
fn a_huge_declared_size_is_shown_smaller_and_drawn_at_the_cap() {
    let ready =
        picture::prepare(svg("width=\"1000000\" height=\"500000\"", "<circle r=\"5\" cx=\"5\" cy=\"5\"/>"), true)
            .unwrap();
    assert_eq!(ready.given, (32_768, 16_384), "scaled to the longest side the model allows");
    assert_eq!(ready.natural, (2048, 1024));
    // A size below one pixel is still a picture of one pixel.
    let tiny = picture::prepare(svg("width=\"0.001\" height=\"0.001\"", ""), true).unwrap();
    assert_eq!(tiny.given, (1, 1));
}

#[test]
fn the_background_stays_clear_and_a_fill_with_opacity_is_not_darkened() {
    let ready = picture::prepare(
        svg(
            "width=\"10\" height=\"10\"",
            "<rect x=\"5\" width=\"5\" height=\"10\" fill=\"#ff0000\" fill-opacity=\"0.5\"/>",
        ),
        true,
    )
    .unwrap();
    let img = pixels(&ready);
    assert_eq!(img.get_pixel(0, 0).0[3], 0, "nothing is drawn on the left half");
    let [r, g, b, a] = img.get_pixel(img.width() - 1, 0).0;
    assert!(
        r >= 254 && g == 0 && b == 0 && (126..=129).contains(&a),
        "straight alpha, not premultiplied: {r} {g} {b} {a}"
    );
}

#[test]
fn text_is_not_drawn_and_the_picture_says_so() {
    let with_text =
        picture::prepare(svg("width=\"10\" height=\"10\"", "<text x=\"1\" y=\"8\">Hi</text>"), true).unwrap();
    assert_eq!(with_text.warnings.len(), 1);
    assert!(with_text.warnings[0].contains("text"), "{:?}", with_text.warnings);
    assert!(pixels(&with_text).pixels().all(|p| p.0[3] == 0));
    assert!(picture::prepare(svg("width=\"10\" height=\"10\"", ""), true).unwrap().warnings.is_empty());
}

#[test]
fn a_bom_a_prolog_and_a_doctype_are_fine() {
    let mut file = vec![0xef, 0xbb, 0xbf];
    file.extend_from_slice(b"<?xml version=\"1.0\"?>\n<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">\n");
    file.extend_from_slice(&svg("width=\"8\" height=\"8\"", "<rect width=\"8\" height=\"8\"/>"));
    assert_eq!(picture::sniff(&file), Some(Kind::Svg));
    assert_eq!(picture::prepare(file, true).unwrap().given, (8, 8));
}

#[test]
fn xml_that_is_not_an_svg_is_refused() {
    assert!(
        why(picture::prepare(b"<html><body><svg width='1' height='1'/></body></html>".to_vec(), true))
            .contains("not `svg`")
    );
    assert!(why(picture::prepare(b"<svg xmlns='http://www.w3.org/2000/svg' width='5'".to_vec(), true)).contains("XML"));
    assert!(
        why(picture::prepare(b"<svg xmlns='http://example.com/other' width='5' height='5'/>".to_vec(), true))
            .contains("not `svg`")
    );
    // No size and no view box: the size of what is drawn.
    assert_eq!(picture::prepare(svg("", "<rect width=\"4\" height=\"4\"/>"), true).unwrap().given, (4, 4));
    assert_eq!(picture::prepare(svg("", ""), true).unwrap().given, (100, 100), "an empty one is the default 100 x 100");
    assert!(why(picture::prepare(svg("width=\"0\" height=\"5\"", ""), true)).contains("size"));
}

// ---- hostile SVG ----

/// A small red PNG written next to the test: what an SVG would read if it could read files.
fn red_png_on_disk() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("drafftink-svg-test-{}.png", std::process::id()));
    std::fs::write(&path, png_of(&RgbaImage::from_pixel(8, 8, RED))).unwrap();
    path
}

fn embedded_png_url() -> String {
    use base64::Engine as _;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png_of(&RgbaImage::from_pixel(8, 8, RED)))
    )
}

#[test]
fn an_svg_cannot_read_files_or_urls() {
    let path = red_png_on_disk();
    let p = path.display();
    let name = path.file_name().unwrap().to_string_lossy();
    let dir = path.parent().unwrap().display();
    // The control: the converter with its own defaults does read the file, so the checks below can fail.
    let attack = svg("width=\"8\" height=\"8\"", &format!("<image href=\"{p}\" width=\"8\" height=\"8\"/>"));
    let tree = resvg::usvg::Tree::from_data(&attack, &resvg::usvg::Options::default()).unwrap();
    assert!(
        tree.root().children().iter().any(|n| matches!(n, resvg::usvg::Node::Group(_))),
        "the default options read {p}"
    );
    // An absolute path, a file URL, a relative name, an http URL, and the same as a feImage and with the href of SVG 2.
    for href in [
        format!("{p}"),
        format!("file://{p}"),
        format!("{name}"),
        "http://127.0.0.1:9/x.png".to_owned(),
        format!("../{name}"),
    ] {
        let body = format!(
            "<image href=\"{href}\" width=\"8\" height=\"8\"/><image xlink:href=\"{href}\" width=\"8\" height=\"8\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"/>"
        );
        let ready = picture::prepare(svg("width=\"8\" height=\"8\"", &body), true).unwrap();
        assert!(pixels(&ready).pixels().all(|p| p.0[3] == 0), "`{href}` was read (the folder is {dir})");
    }
    let filter = format!(
        "<filter id=\"f\"><feImage href=\"{p}\" width=\"8\" height=\"8\"/></filter><rect width=\"8\" height=\"8\" filter=\"url(#f)\"/>"
    );
    let ready = picture::prepare(svg("width=\"8\" height=\"8\"", &filter), true).unwrap();
    assert!(pixels(&ready).pixels().all(|p| p.0[3] == 0), "feImage read a file");
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn a_picture_inside_an_svg_is_drawn_when_it_is_a_small_data_url_and_not_otherwise() {
    let draw = |href: &str| {
        let body = format!("<image href=\"{href}\" width=\"8\" height=\"8\"/>");
        pixels(&picture::prepare(svg("width=\"8\" height=\"8\"", &body), true).unwrap())
    };
    assert!(draw(&embedded_png_url()).pixels().all(|p| *p == RED), "the control: a PNG data URL is drawn");
    // The content decides, not the media type: this is not a picture.
    assert!(draw("data:image/png;base64,aGVsbG8=").pixels().all(|p| p.0[3] == 0));
    // An SVG inside an SVG is not drawn, whatever it holds.
    let inner =
        "<svg xmlns='http://www.w3.org/2000/svg' width='8' height='8'><rect width='8' height='8' fill='red'/></svg>";
    let nested = format!("data:image/svg+xml;utf8,{}", inner.replace('<', "%3C").replace('>', "%3E"));
    assert!(draw(&nested).pixels().all(|p| p.0[3] == 0));
    use base64::Engine as _;
    let nested = format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(inner));
    assert!(draw(&nested).pixels().all(|p| p.0[3] == 0));
}

#[test]
fn a_picture_inside_an_svg_that_claims_a_huge_size_is_not_decoded() {
    use base64::Engine as _;
    let mut png = png_of(&RgbaImage::from_pixel(8, 8, RED));
    png[16..20].copy_from_slice(&30_000u32.to_be_bytes());
    png[20..24].copy_from_slice(&30_000u32.to_be_bytes());
    let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
    let body = format!("<image href=\"{url}\" width=\"8\" height=\"8\"/>");
    let ready = picture::prepare(svg("width=\"8\" height=\"8\"", &body), true).unwrap();
    assert!(pixels(&ready).pixels().all(|p| p.0[3] == 0));
}

#[test]
fn scripts_event_handlers_and_stylesheet_imports_are_inert() {
    let file = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8" onload="alert(1)">
        <script>document.location='http://example.com'</script>
        <style>@import url(http://127.0.0.1:9/a.css); rect { fill: #ff0000 }</style>
        <a href="javascript:alert(1)"><rect width="8" height="8" onclick="alert(2)"/></a>
        <foreignObject width="8" height="8"><div xmlns="http://www.w3.org/1999/xhtml">x</div></foreignObject>
    </svg>"#;
    let ready = picture::prepare(file.to_vec(), true).unwrap();
    assert!(pixels(&ready).pixels().all(|p| *p == RED), "the rectangle is drawn, nothing else happens");
}

#[test]
fn deeply_nested_elements_are_refused_without_a_stack_overflow() {
    let nested = |n: usize| {
        svg(
            "width=\"8\" height=\"8\"",
            &format!("{}<rect width=\"8\" height=\"8\"/>{}", "<g>".repeat(n), "</g>".repeat(n)),
        )
    };
    assert!(picture::prepare(nested(100), true).is_ok(), "100 levels are fine");
    assert!(why(picture::prepare(nested(200), true)).contains("deep"));
    assert!(why(picture::prepare(nested(100_000), true)).contains("deep"));
    assert!(why(picture::prepare(nested(300_000), true)).contains("more than"), "too many elements before too deep");
}

#[test]
fn use_chains_that_multiply_are_refused_before_they_are_copied() {
    // Ten levels, each a group of ten uses of the one below: 10^10 rectangles.
    let mut defs = String::from("<rect id=\"g0\" width=\"1\" height=\"1\"/>");
    for level in 1..=10 {
        let uses = format!("<use href=\"#g{}\"/>", level - 1).repeat(10);
        defs.push_str(&format!("<g id=\"g{level}\">{uses}</g>"));
    }
    let bomb = svg("width=\"8\" height=\"8\"", &format!("<defs>{defs}</defs><use href=\"#g10\"/>"));
    assert!(why(picture::prepare(bomb, true)).contains("use"));
    // Two levels of ten are only a hundred copies.
    let mut fine = String::from("<rect id=\"g0\" width=\"1\" height=\"1\"/>");
    for level in 1..=2 {
        fine.push_str(&format!("<g id=\"g{level}\">{}</g>", format!("<use href=\"#g{}\"/>", level - 1).repeat(10)));
    }
    let fine = svg("width=\"8\" height=\"8\"", &format!("<defs>{fine}</defs><use href=\"#g2\"/>"));
    assert!(picture::prepare(fine, true).is_ok());
    // A use that points at its own parent never ends.
    let cycle = svg("width=\"8\" height=\"8\"", "<g id=\"a\"><use href=\"#a\"/></g>");
    assert!(why(picture::prepare(cycle, true)).contains("use"));
}

#[test]
fn entity_bombs_and_huge_documents_are_refused() {
    let mut doctype = String::from("<!DOCTYPE svg [<!ENTITY a0 \"ha\">");
    for i in 1..10 {
        doctype.push_str(&format!("<!ENTITY a{i} \"{}\">", format!("&a{};", i - 1).repeat(10)));
    }
    doctype.push_str("]>");
    let bomb = format!("{doctype}<svg {SVG_NS} width=\"8\" height=\"8\"><title>&a9;</title></svg>");
    assert!(why(picture::prepare(bomb.into_bytes(), true)).contains("XML"));
    // Too many elements, and too many bytes.
    let many = svg("width=\"8\" height=\"8\"", &"<g/>".repeat(300_000));
    assert!(why(picture::prepare(many, true)).contains("more than"));
    let mut heavy = svg("width=\"8\" height=\"8\"", "");
    heavy.extend(std::iter::repeat_n(b' ', 9 * 1024 * 1024));
    assert!(why(picture::prepare(heavy, true)).contains("bytes"));
}

#[test]
fn a_pattern_tile_of_a_huge_size_is_refused() {
    let pattern = |size: u32| {
        svg(
            "width=\"8\" height=\"8\"",
            &format!(
                "<defs><pattern id=\"p\" patternUnits=\"userSpaceOnUse\" width=\"{size}\" height=\"{size}\"><rect width=\"2\" height=\"2\"/></pattern></defs><rect width=\"8\" height=\"8\" fill=\"url(#p)\"/>"
            ),
        )
    };
    assert!(why(picture::prepare(pattern(1_000_000), true)).contains("pattern"));
    assert!(picture::prepare(pattern(4), true).is_ok(), "a small pattern is drawn");
    // The same through a scale on the element that uses it.
    let scaled = svg(
        "width=\"8\" height=\"8\"",
        "<defs><pattern id=\"p\" patternUnits=\"userSpaceOnUse\" width=\"4\" height=\"4\"><rect width=\"2\" height=\"2\"/></pattern></defs><g transform=\"scale(100000)\"><rect width=\"8\" height=\"8\" fill=\"url(#p)\"/></g>",
    );
    assert!(why(picture::prepare(scaled, true)).contains("pattern"));
    // And through a mask, which is drawn like a group.
    let masked = svg(
        "width=\"8\" height=\"8\"",
        "<defs><pattern id=\"p\" patternUnits=\"userSpaceOnUse\" width=\"1000000\" height=\"1000000\"><rect width=\"2\" height=\"2\"/></pattern><mask id=\"m\"><rect width=\"8\" height=\"8\" fill=\"url(#p)\"/></mask></defs><rect width=\"8\" height=\"8\" mask=\"url(#m)\"/>",
    );
    assert!(why(picture::prepare(masked, true)).contains("pattern"));
}

#[test]
fn damaged_and_cut_svg_never_panic() {
    let good = svg(
        "width=\"20\" height=\"10\" viewBox=\"0 0 20 10\"",
        "<defs><linearGradient id=\"g\"><stop offset=\"0\" stop-color=\"red\"/><stop offset=\"1\" stop-color=\"blue\"/></linearGradient></defs><rect width=\"20\" height=\"10\" fill=\"url(#g)\" stroke=\"black\" stroke-dasharray=\"1 2\"/><path d=\"M0 0 L20 10 Q5 5 1 9z\"/>",
    );
    for i in (0..good.len()).step_by(3) {
        let mut flipped = good.clone();
        flipped[i] ^= 0x55;
        let _ = picture::prepare(flipped, true);
        let _ = picture::prepare(good[..i].to_vec(), true);
    }
}
