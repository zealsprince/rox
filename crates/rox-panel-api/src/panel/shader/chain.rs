//! The `// @pass` and `// @asset` splitter, and the one call every shader
//! surface registers through.
//!
//! A rox shader is one WGSL text (ADR 23), so the chain it describes is
//! expressed in comment directives rather than in the config: the pool entry,
//! the eject file, the hot reload watch, the approval fingerprint and the
//! bundle all assume one shader is one text, and a pass array would fork
//! every one of them. This is the reader for that, next to
//! [`slot_labels`](super::slot_labels), which established the convention.
//!
//! Everything a text declares resolves here too: an `// @asset` line names
//! an image, and the bytes come from the pool entry the source resolved
//! from or from a file beside it, never from a path the text itself picked.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{UserShaderChain, UserShaderId, UserShaderPass, Window};

/// How many passes and images one program may declare, the same caps the
/// window API enforces. Past this the design being expressed needs a render
/// graph, which ADR 23 rules out.
const MAX_PASSES: usize = 8;
const MAX_ASSETS: usize = 8;

/// The only scales a pass may render at, so the renderer's target sizes stay
/// predictable. Halves all the way down, for pyramid work.
const SCALES: [f32; 4] = [1.0, 0.5, 0.25, 0.125];

/// The names the wrapping template already binds, which a pass or an image
/// can't take.
const RESERVED: [&str; 5] = ["params", "screen", "samp", "prev", "mask"];

/// The one dynamic image source: `// @asset art: @cover` binds the playing
/// track's cover instead of a file. The bytes come from the window's cover
/// feed at registration, and a track without art gets [`fallback_cover`],
/// so the binding always samples something.
pub const COVER_SOURCE: &str = "@cover";

/// One fragment stage of a program: what later passes bind its output
/// under, the WGSL that runs, and the fraction of the surface it renders at.
#[derive(Clone, Debug, PartialEq)]
pub struct PassSpec {
    pub name: String,
    /// The shared prelude followed by this pass's own section, which is the
    /// module the window compiles.
    pub body: String,
    pub scale: f32,
}

/// One image a program declares: the name it binds under and the file it
/// comes from.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetRef {
    pub name: String,
    /// A flat file name, no separators. It's the key into a pool entry's
    /// bundled assets as much as it is a file beside the source. The one
    /// value that isn't a file is [`COVER_SOURCE`].
    pub file: String,
}

impl AssetRef {
    /// Whether this binding is the playing track's art rather than a file.
    pub fn is_cover(&self) -> bool {
        self.file == COVER_SOURCE
    }
}

/// What a shader text describes: an ordered chain of passes and the images
/// they may sample.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainSpec {
    pub passes: Vec<PassSpec>,
    pub assets: Vec<AssetRef>,
}

impl ChainSpec {
    /// Whether this is a text with nothing to split: one pass, no images,
    /// which is every shader that existed before chains. Those go straight
    /// down the old registration path with their original text, so the
    /// splitter can't change what a shader that never asked for it compiles
    /// to.
    pub fn plain(&self) -> bool {
        self.assets.is_empty() && self.passes.len() == 1 && self.passes[0].name == "main"
    }

    /// Whether any binding is the playing track's art, which makes a
    /// program's identity move with the track.
    pub fn wants_cover(&self) -> bool {
        self.assets.iter().any(AssetRef::is_cover)
    }
}

/// Whether a text mentions [`COVER_SOURCE`] at all: the cheap per-frame
/// check the surfaces gate their cover polling on, so a shader that never
/// asked for art costs nothing on a track change. A mention in prose reads
/// as true too, which only re-keys that program when the track turns over:
/// a spurious registration there, never a missing one.
pub fn uses_cover(source: &str) -> bool {
    source.contains(COVER_SOURCE)
}

/// Whether a text mentions the `mask` binding at all: the cheap check the
/// panel wrapper gates its span brackets on, ahead of registration ever
/// running. The same over-approximation as [`uses_cover`]: a mention in
/// prose records a span nothing reads, two marker entries and no capture,
/// while a real read is never missed.
pub fn uses_mask(source: &str) -> bool {
    source.contains("mask")
}

/// Read the chain out of a shader text.
///
/// Scanned line by line the way `// @slot n: name` is: trim, strip the
/// comment marker, trim, match the keyword. Text above the first `// @pass`
/// is a prelude prepended to every pass, so helpers and constants are
/// written once. A text with no `// @pass` in it is one pass called `main`
/// holding the whole thing, which is why nothing migrates.
///
/// Errors name the line they came from and read like the naga messages
/// they appear beside in the same three readouts.
pub fn parse_chain(source: &str) -> Result<ChainSpec, String> {
    let mut passes: Vec<PassSpec> = Vec::new();
    let mut assets: Vec<AssetRef> = Vec::new();
    let mut prelude = String::new();
    let mut declared: HashSet<String> = HashSet::new();

    for (index, line) in source.lines().enumerate() {
        let number = index + 1;
        if let Some(rest) = directive(line, "@pass") {
            let (name, tail) = split_tail(rest);
            if name.is_empty() {
                return Err(format!(
                    "line {number}: @pass needs a name, as `// @pass blur`"
                ));
            }
            claim(&mut declared, name, number)?;
            let scale = match tail {
                None => 1.0,
                Some(text) => parse_scale(text, name, number)?,
            };
            passes.push(PassSpec {
                name: name.to_string(),
                body: String::new(),
                scale,
            });
            // The cut line itself belongs to neither side.
            continue;
        }
        if let Some(rest) = directive(line, "@asset") {
            let (name, tail) = split_tail(rest);
            let Some(file) = tail.filter(|file| !file.is_empty()) else {
                return Err(format!(
                    "line {number}: @asset needs a name and a file, as `// @asset plate: plate.png`"
                ));
            };
            if name.is_empty() {
                return Err(format!(
                    "line {number}: @asset needs a name and a file, as `// @asset plate: plate.png`"
                ));
            }
            claim(&mut declared, name, number)?;
            if file.starts_with('@') && file != COVER_SOURCE {
                return Err(format!(
                    "line {number}: asset '{name}': `{file}` isn't a source rox provides; \
                     the only one is {COVER_SOURCE}"
                ));
            }
            if file.contains(['/', '\\']) || file == "." || file == ".." {
                return Err(format!(
                    "line {number}: asset '{name}': `{file}` has to be a plain file name, since \
                     it's read from the shader's own folder"
                ));
            }
            assets.push(AssetRef {
                name: name.to_string(),
                file: file.to_string(),
            });
            // An `@asset` line is a comment wherever it appears, so it stays in
            // the text rather than being cut out of it.
        }
        match passes.last_mut() {
            Some(pass) => {
                pass.body.push_str(line);
                pass.body.push('\n');
            }
            None => {
                prelude.push_str(line);
                prelude.push('\n');
            }
        }
    }

    if passes.len() > MAX_PASSES {
        return Err(format!(
            "a shader program is capped at {MAX_PASSES} passes, found {}",
            passes.len()
        ));
    }
    if assets.len() > MAX_ASSETS {
        return Err(format!(
            "a shader program is capped at {MAX_ASSETS} images, found {}",
            assets.len()
        ));
    }

    if passes.is_empty() {
        // No cut points, so the whole text is the one pass and there's no
        // prelude to prepend: it would only be the same lines twice.
        passes.push(PassSpec {
            name: "main".to_string(),
            body: source.to_string(),
            scale: 1.0,
        });
    } else {
        if !prelude.trim().is_empty() {
            for pass in &mut passes {
                pass.body = format!("{prelude}{}", pass.body);
            }
        }
        // The last pass draws the result, so it has nowhere to be scaled to.
        let last = passes.last().expect("a pass was just pushed");
        if last.scale != 1.0 {
            return Err(format!(
                "pass '{}': the final pass draws the result, so it has to be full size",
                last.name
            ));
        }
    }

    Ok(ChainSpec { passes, assets })
}

/// A directive's tail, or None when the line isn't one. A keyword followed
/// by more word (`// @passing thought`) is prose, not a directive.
pub(super) fn directive<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix("//")?;
    let rest = rest.trim_start().strip_prefix(keyword)?;
    let next = rest.chars().next();
    match next {
        None => Some(rest),
        Some(c) if c.is_whitespace() || c == ':' => Some(rest),
        Some(_) => None,
    }
}

/// A directive's name and whatever followed the colon.
fn split_tail(rest: &str) -> (&str, Option<&str>) {
    match rest.split_once(':') {
        Some((name, tail)) => (name.trim(), Some(tail.trim())),
        None => (rest.trim(), None),
    }
}

/// Take a name for this program, or report why it can't be. Passes and
/// images share one namespace because they end up as bindings in the same
/// module.
fn claim(declared: &mut HashSet<String>, name: &str, number: usize) -> Result<(), String> {
    if !binding_name(name) {
        return Err(format!(
            "line {number}: `{name}` isn't a usable binding name"
        ));
    }
    if RESERVED.contains(&name) {
        return Err(format!(
            "line {number}: `{name}` is one of the template's own bindings, pick another name"
        ));
    }
    if !declared.insert(name.to_string()) {
        return Err(format!("line {number}: `{name}` is declared twice"));
    }
    Ok(())
}

/// A WGSL identifier, which a name has to be to compose into the module.
fn binding_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn parse_scale(text: &str, name: &str, number: usize) -> Result<f32, String> {
    let scale: f32 = text
        .parse()
        .map_err(|_| format!("line {number}: pass '{name}': scale `{text}` isn't a number"))?;
    if !SCALES.contains(&scale) {
        return Err(format!(
            "line {number}: pass '{name}': scale {scale} isn't one of 1.0, 0.5, 0.25, 0.125"
        ));
    }
    Ok(scale)
}

/// Where a program's images may be read from: the workspace shader the
/// source resolved from, and the file it was read from. Both are optional
/// and both are only ever a place to look, never something the shader text
/// gets to name.
///
/// A text that declares an image and has neither of these is detached, and
/// registration reports that rather than guessing: an inline source that
/// arrived in a layout has nothing on this machine to hold its plates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgramCtx {
    /// The pool entry the source came from, whose bundled assets win.
    pub name: Option<String>,
    /// The file the source was read from; its siblings are the fallback,
    /// which makes the eject-and-edit loop work for images too.
    pub path: Option<PathBuf>,
}

impl ProgramCtx {
    /// A source with nothing behind it: an inline shader out of a layout.
    pub fn detached() -> ProgramCtx {
        ProgramCtx::default()
    }

    /// A source resolved from the workspace's shader pool.
    pub fn named(name: impl Into<String>) -> ProgramCtx {
        ProgramCtx {
            name: Some(name.into()),
            path: None,
        }
    }

    /// A source read from a file on this machine.
    pub fn file(path: impl Into<PathBuf>) -> ProgramCtx {
        ProgramCtx {
            name: None,
            path: Some(path.into()),
        }
    }

    /// What the surface drivers hold: a config's pool name and its file
    /// bookmark, either of which may be absent.
    pub fn of(name: Option<&str>, path: Option<&Path>) -> ProgramCtx {
        ProgramCtx {
            name: name.map(str::to_string),
            path: path.map(Path::to_path_buf),
        }
    }
}

/// One decoded image, ready for [`Window::register_user_texture`].
#[derive(Clone, Debug, PartialEq)]
pub struct AssetImage {
    pub width: u32,
    pub height: u32,
    /// Straight-alpha rgba8, `width * height * 4` bytes, treated as sRGB.
    pub rgba8: Vec<u8>,
}

/// Find and decode every image a chain declares, in declaration order.
///
/// The pool entry's bundled bytes win, since those are what travelled with
/// the look; a file beside the source is the fallback, which is how an edit
/// made in an image editor gets picked up. Errors name the image the way naga names
/// the pass, so a broken plate reads like a broken shader in the same
/// readout.
///
/// `cover` is the playing track's art, already decoded, for the bindings
/// that declared [`COVER_SOURCE`]; None means no track or no art, which
/// binds [`fallback_cover`] rather than failing, since "nothing playing"
/// is a state every session passes through.
pub fn resolve_assets(
    spec: &ChainSpec,
    ctx: &ProgramCtx,
    cover: Option<&AssetImage>,
) -> Result<Vec<(String, AssetImage)>, String> {
    if spec.assets.is_empty() {
        return Ok(Vec::new());
    }
    let entry = ctx
        .name
        .as_deref()
        .and_then(rox_core::settings::shader_pool_get);
    // The cover comes from the player, not from a folder, so a program
    // binding nothing but art runs fine detached: inline in a layout,
    // pasted into the editor, anywhere.
    let file_backed = spec.assets.iter().find(|asset| !asset.is_cover());
    if let Some(first) = file_backed.filter(|_| entry.is_none() && ctx.path.is_none()) {
        return Err(format!(
            "asset '{}': a shader that declares an image has to come from this workspace's \
             shaders or from a file, so the bytes have somewhere to live",
            first.name
        ));
    }

    let mut images = Vec::with_capacity(spec.assets.len());
    for asset in &spec.assets {
        if asset.is_cover() {
            let image = cover.cloned().unwrap_or_else(fallback_cover);
            images.push((asset.name.clone(), image));
            continue;
        }
        let bytes = carried(entry.as_ref(), &asset.file)
            .or_else(|| beside(ctx.path.as_deref(), &asset.file))
            .or_else(|| beside(entry.as_ref().and_then(|e| e.path.as_deref()), &asset.file))
            .transpose()
            .map_err(|err| format!("asset '{}': {err}", asset.name))?;
        let Some(bytes) = bytes else {
            return Err(format!(
                "asset '{}': {} isn't in this workspace's shaders or beside the source",
                asset.name, asset.file
            ));
        };
        let image = decode(&bytes).map_err(|err| format!("asset '{}': {err}", asset.name))?;
        images.push((asset.name.clone(), image));
    }
    Ok(images)
}

/// The bytes a pool entry holds for a file name, if it holds that one.
fn carried(
    entry: Option<&rox_core::settings::NamedShader>,
    file: &str,
) -> Option<Result<Vec<u8>, String>> {
    entry?
        .assets
        .iter()
        .find(|asset| asset.file == file)
        .map(|asset| asset.decode())
}

/// The bytes of a file next to a source file.
fn beside(source: Option<&Path>, file: &str) -> Option<Result<Vec<u8>, String>> {
    let path = source?.parent()?.join(file);
    if !path.exists() {
        return None;
    }
    Some(std::fs::read(&path).map_err(|err| err.to_string()))
}

/// What a [`COVER_SOURCE`] binding samples when nothing plays or the track
/// has no art: a flat dark plate, so the shader's math runs over
/// something instead of the registration failing. Opaque and near-black, so
/// the common uses (sorting, quantizing, dissolving the art) degrade to a
/// quiet nothing rather than a white flash.
pub fn fallback_cover() -> AssetImage {
    const EDGE: usize = 8;
    AssetImage {
        width: EDGE as u32,
        height: EDGE as u32,
        rgba8: [26, 26, 26, 255].repeat(EDGE * EDGE),
    }
}

/// An encoded image file as pixels. Straight alpha, which the window API
/// takes.
pub(crate) fn decode(bytes: &[u8]) -> Result<AssetImage, String> {
    let image = image::load_from_memory(bytes)
        .map_err(|err| err.to_string())?
        .to_rgba8();
    Ok(AssetImage {
        width: image.width(),
        height: image.height(),
        rgba8: image.into_raw(),
    })
}

/// Register a whole shader program with a window: split the text, find its
/// images, upload them, and hand the chain over. Every shader surface calls
/// this, so all three of them grew chains and assets at once.
///
/// A text with no directives in it takes the old single-source path
/// verbatim, so the shaders that already run keep compiling to exactly what
/// they compiled to before.
pub fn register_program(
    window: &mut Window,
    source: &str,
    ctx: &ProgramCtx,
) -> Result<UserShaderId, String> {
    let spec = parse_chain(source)?;
    if spec.plain() {
        return window.register_user_shader(source);
    }
    // The window's cover feed, kept current by the surface drivers' polls;
    // fetched only when a binding declared one, so most programs never touch it.
    let cover = spec
        .wants_cover()
        .then(|| super::window_cover(window.window_handle().window_id().as_u64()))
        .flatten();
    let images = resolve_assets(&spec, ctx, cover.as_deref())?;
    let mut assets = Vec::with_capacity(images.len());
    for (name, image) in images {
        let id = window
            .register_user_texture(image.width, image.height, &image.rgba8)
            .map_err(|err| format!("asset '{name}': {err}"))?;
        assets.push((name, id));
    }
    let chain = UserShaderChain {
        passes: spec
            .passes
            .iter()
            .map(|pass| UserShaderPass {
                name: pass.name.clone(),
                source: pass.body.clone(),
                scale: pass.scale,
            })
            .collect(),
        assets,
    };
    window.register_user_shader_chain(&chain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_core::settings::{NamedShader, ShaderAsset};

    const FS_USER: &str = "fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";

    /// Every shader that exists today has no directives, and comes back as
    /// one pass holding its own text, untouched.
    #[test]
    fn a_text_with_no_directives_is_one_pass() {
        let spec = parse_chain(FS_USER).expect("parse");
        assert_eq!(spec.passes.len(), 1);
        assert_eq!(spec.passes[0].name, "main");
        assert_eq!(spec.passes[0].body, FS_USER);
        assert_eq!(spec.passes[0].scale, 1.0);
        assert!(spec.assets.is_empty());
        assert!(spec.plain(), "and it takes the old registration path");

        // The other conventions in a shader's comments aren't cut points.
        let slotted = format!("// @slot 0: bass\n// @passing thought\n{FS_USER}");
        let spec = parse_chain(&slotted).expect("parse");
        assert_eq!(spec.passes.len(), 1);
        assert_eq!(spec.passes[0].body, slotted);
    }

    /// The cut, the shared prelude, and the scales.
    #[test]
    fn passes_cut_the_text_and_carry_the_prelude() {
        let source = "\
const K: f32 = 2.0;
// @pass down: 0.5
fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(K); }
// @pass up
fn fs_user(uv: vec2<f32>) -> vec4<f32> { return textureSample(down, samp, uv); }";
        let spec = parse_chain(source).expect("parse");
        assert!(!spec.plain());
        assert_eq!(spec.passes.len(), 2);
        assert_eq!(spec.passes[0].name, "down");
        assert_eq!(spec.passes[0].scale, 0.5);
        assert_eq!(spec.passes[1].name, "up");
        assert_eq!(spec.passes[1].scale, 1.0);
        // The prelude leads every pass, and the cut lines belong to neither.
        for pass in &spec.passes {
            assert!(
                pass.body.starts_with("const K: f32 = 2.0;\n"),
                "{}",
                pass.body
            );
            assert!(!pass.body.contains("@pass"), "{}", pass.body);
        }
        assert!(spec.passes[0].body.contains("vec4<f32>(K)"));
        assert!(spec.passes[1].body.contains("textureSample(down"));
        assert!(!spec.passes[1].body.contains("vec4<f32>(K)"));
    }

    /// What the grammar rejects, since these all end up in a readout
    /// somebody has to act on.
    #[test]
    fn the_grammar_says_why_it_said_no() {
        let err = |source: &str| parse_chain(source).expect_err("should refuse");

        assert!(err("// @pass\n").contains("needs a name"));
        assert!(err("// @pass 2fast\n").contains("isn't a usable binding name"));
        assert!(err("// @pass screen\n").contains("template's own bindings"));
        assert!(
            err("// @pass blur\n// @pass blur\n").contains("declared twice"),
            "two passes can't share a name"
        );
        assert!(
            err("// @pass blur\n// @asset blur: plate.png\n").contains("declared twice"),
            "and a pass and an image share one namespace"
        );

        // Scales are a fixed set, and the final pass draws the result so it
        // has nowhere to be scaled to.
        assert!(err("// @pass a: 0.3\n// @pass b\n").contains("isn't one of"));
        assert!(err("// @pass a: half\n// @pass b\n").contains("isn't a number"));
        assert!(err("// @pass a: 0.5\n").contains("has to be full size"));

        // The caps, which are where the design wants a render graph instead.
        let many: String = (0..9).map(|n| format!("// @pass p{n}\n")).collect();
        assert!(err(&many).contains("capped at 8 passes"));
        let images: String = (0..9)
            .map(|n| format!("// @asset a{n}: {n}.png\n"))
            .collect();
        assert!(err(&images).contains("capped at 8 images"));

        // An image is read from the shader's own folder, so its name can't
        // escape it.
        assert!(err("// @asset plate: ../../secrets.png\n").contains("plain file name"));
        assert!(err("// @asset plate:\n").contains("needs a name and a file"));
    }

    /// A 2x2 PNG, small enough to inline in a test and real enough to decode.
    fn plate() -> Vec<u8> {
        let mut image = image::RgbaImage::new(2, 2);
        image.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode");
        bytes.into_inner()
    }

    /// A scratch folder of this test's own, so a parallel run can't read
    /// somebody else's writes.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-shader-assets-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The three places an image can come from, and the one case where
    /// there's nowhere to look.
    #[test]
    fn an_image_resolves_from_the_pool_or_from_beside_the_source() {
        let source = format!("// @asset plate: plate.png\n{FS_USER}");
        let spec = parse_chain(&source).expect("parse");
        assert_eq!(spec.assets.len(), 1);
        assert_eq!(spec.assets[0].name, "plate");
        assert_eq!(spec.assets[0].file, "plate.png");

        // Nothing behind the text: the bytes have nowhere to come from, and
        // saying so beats registering a shader that samples a hole.
        let detached = resolve_assets(&spec, &ProgramCtx::detached(), None).expect_err("detached");
        assert!(detached.contains("asset 'plate'"), "{detached}");
        assert!(detached.contains("from a file"), "{detached}");

        // The pool entry's own bytes, which travelled with a look.
        let _pool = crate::panel::shader::POOL_GUARD
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        rox_core::settings::note_shader_pool(vec![NamedShader {
            name: "Stamp".to_string(),
            source: source.clone(),
            path: None,
            assets: vec![ShaderAsset::from_bytes("plate.png", &plate())],
        }]);
        let carried =
            resolve_assets(&spec, &ProgramCtx::named("Stamp"), None).expect("from the pool");
        assert_eq!(carried.len(), 1);
        assert_eq!(carried[0].0, "plate");
        assert_eq!(carried[0].1.width, 2);
        assert_eq!(carried[0].1.height, 2);
        assert_eq!(carried[0].1.rgba8.len(), 2 * 2 * 4);

        // A file beside the source, which is the authoring loop's half.
        let dir = scratch("resolve");
        let wgsl = dir.join("stamp.wgsl");
        std::fs::write(&wgsl, &source).expect("write");
        std::fs::write(dir.join("plate.png"), plate()).expect("write");
        let sibling =
            resolve_assets(&spec, &ProgramCtx::file(&wgsl), None).expect("from the folder");
        assert_eq!(sibling[0].1.width, 2);

        // A file the folder doesn't hold reads as missing rather than as a
        // decode failure.
        let empty = scratch("resolve-empty");
        let missing = resolve_assets(&spec, &ProgramCtx::file(empty.join("stamp.wgsl")), None)
            .expect_err("nothing there");
        assert!(missing.contains("plate.png isn't"), "{missing}");

        // And something that isn't an image at all reads out the way a
        // broken shader does.
        std::fs::write(dir.join("plate.png"), b"not a png").expect("write");
        let broken =
            resolve_assets(&spec, &ProgramCtx::file(&wgsl), None).expect_err("not an image");
        assert!(broken.starts_with("asset 'plate':"), "{broken}");

        rox_core::settings::note_shader_pool(Vec::new());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&empty).ok();
    }

    /// A text with no images never looks anything up, so a detached shader
    /// stays as cheap as it was.
    #[test]
    fn a_text_with_no_images_asks_nothing_of_its_context() {
        let spec = parse_chain(FS_USER).expect("parse");
        assert!(resolve_assets(&spec, &ProgramCtx::detached(), None)
            .expect("nothing to find")
            .is_empty());
    }

    /// `@cover` binds the playing track's art: it needs no folder behind
    /// the text, takes the image the feed hands over, and falls back to the
    /// dark plate when nothing plays or the track has none.
    #[test]
    fn a_cover_binding_comes_from_the_player_not_a_folder() {
        let source = format!("// @asset art: @cover\n{FS_USER}");
        let spec = parse_chain(&source).expect("parse");
        assert!(spec.wants_cover());
        assert!(spec.assets[0].is_cover());
        assert!(uses_cover(&source));
        assert!(!uses_cover(FS_USER));

        // Detached is fine: the art comes from the player, so an inline
        // shader out of a layout binds it the same as an ejected one.
        let bound = resolve_assets(&spec, &ProgramCtx::detached(), None).expect("fallback");
        assert_eq!(bound.len(), 1);
        assert_eq!(bound[0].0, "art");
        assert_eq!(bound[0].1, fallback_cover());

        // With art on the feed, the binding takes it as handed over.
        let art = decode(&plate()).expect("decode");
        let bound = resolve_assets(&spec, &ProgramCtx::detached(), Some(&art)).expect("cover");
        assert_eq!(bound[0].1, art);

        // A file asset beside a cover still needs somewhere to live, and
        // the error names the file one rather than the cover.
        let both = format!("// @asset art: @cover\n// @asset plate: plate.png\n{FS_USER}");
        let spec = parse_chain(&both).expect("parse");
        let err = resolve_assets(&spec, &ProgramCtx::detached(), None).expect_err("detached");
        assert!(err.contains("asset 'plate'"), "{err}");

        // And an @-name that isn't the cover reads out at parse, not as a
        // file that happens not to exist.
        let bogus =
            parse_chain(&format!("// @asset art: @screen\n{FS_USER}")).expect_err("not a source");
        assert!(bogus.contains("@cover"), "{bogus}");
    }
}
