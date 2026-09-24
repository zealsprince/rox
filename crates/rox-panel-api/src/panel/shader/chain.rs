//! The `// @pass` and `// @asset` splitter, and the one call every shader
//! surface registers through.
//!
//! A rox shader is one WGSL text (ADR 23), so its chain lives in comment
//! directives rather than config: the pool, eject, hot reload, approval
//! fingerprint and bundle all assume one shader is one text.
//!
//! An `// @asset` image's bytes come from the pool entry or a file beside
//! the source, never from a path the text itself picked.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{UserShaderChain, UserShaderId, UserShaderPass, Window};

/// The window API's own caps. Past this a design needs a render graph,
/// which ADR 23 rules out.
const MAX_PASSES: usize = 8;
const MAX_ASSETS: usize = 8;

const SCALES: [f32; 4] = [1.0, 0.5, 0.25, 0.125];

/// The names the wrapping template already binds.
const RESERVED: [&str; 5] = ["params", "screen", "samp", "prev", "mask"];

/// `// @asset art: @cover` binds the playing track's cover instead of a
/// file. A track without art gets [`fallback_cover`].
pub const COVER_SOURCE: &str = "@cover";

#[derive(Clone, Debug, PartialEq)]
pub struct PassSpec {
    pub name: String,
    /// The shared prelude followed by this pass's own section.
    pub body: String,
    pub scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssetRef {
    pub name: String,
    /// A flat file name, no separators: the key into a pool entry's assets
    /// and a file beside the source. Or [`COVER_SOURCE`].
    pub file: String,
}

impl AssetRef {
    pub fn is_cover(&self) -> bool {
        self.file == COVER_SOURCE
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChainSpec {
    pub passes: Vec<PassSpec>,
    pub assets: Vec<AssetRef>,
}

impl ChainSpec {
    /// One pass, no images. These register verbatim, so the splitter can't
    /// change what a shader that never asked for it compiles to.
    pub fn plain(&self) -> bool {
        self.assets.is_empty() && self.passes.len() == 1 && self.passes[0].name == "main"
    }

    /// A cover binding makes the program's identity move with the track.
    pub fn wants_cover(&self) -> bool {
        self.assets.iter().any(AssetRef::is_cover)
    }
}

/// The cheap per-frame check that gates cover polling. A mention in prose
/// reads as true, which costs a spurious re-key, never a missed one.
pub fn uses_cover(source: &str) -> bool {
    source.contains(COVER_SOURCE)
}

/// Gates the panel wrapper's mask span brackets. Over-approximates like
/// [`uses_cover`].
pub fn uses_mask(source: &str) -> bool {
    source.contains("mask")
}

/// Read the chain out of a shader text. Text above the first `// @pass` is
/// a prelude prepended to every pass. A text with no `// @pass` is one pass
/// called `main`, which is why nothing migrates.
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
            // An `@asset` line is a comment, so it stays in the text.
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

/// A directive's tail. A keyword followed by more word (`// @passing
/// thought`) is prose, not a directive.
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

fn split_tail(rest: &str) -> (&str, Option<&str>) {
    match rest.split_once(':') {
        Some((name, tail)) => (name.trim(), Some(tail.trim())),
        None => (rest.trim(), None),
    }
}

/// Passes and images share one namespace: both become bindings in the
/// same module.
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

/// Where a program's images may be read from. Never something the shader
/// text gets to name. With neither set, a text declaring an image is
/// detached and registration reports it rather than guessing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgramCtx {
    /// The pool entry the source came from, whose bundled assets win.
    pub name: Option<String>,
    /// The source file; its siblings are the fallback.
    pub path: Option<PathBuf>,
}

impl ProgramCtx {
    /// An inline shader out of a layout.
    pub fn detached() -> ProgramCtx {
        ProgramCtx::default()
    }

    pub fn named(name: impl Into<String>) -> ProgramCtx {
        ProgramCtx {
            name: Some(name.into()),
            path: None,
        }
    }

    pub fn file(path: impl Into<PathBuf>) -> ProgramCtx {
        ProgramCtx {
            name: None,
            path: Some(path.into()),
        }
    }

    pub fn of(name: Option<&str>, path: Option<&Path>) -> ProgramCtx {
        ProgramCtx {
            name: name.map(str::to_string),
            path: path.map(Path::to_path_buf),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssetImage {
    pub width: u32,
    pub height: u32,
    /// Straight-alpha rgba8, `width * height * 4` bytes, treated as sRGB.
    pub rgba8: Vec<u8>,
}

/// Find and decode every image a chain declares, in declaration order.
/// The pool entry's bytes win over a file beside the source. A `cover` of
/// None binds [`fallback_cover`] rather than failing.
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
    // A program binding nothing but the cover runs fine detached.
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

fn beside(source: Option<&Path>, file: &str) -> Option<Result<Vec<u8>, String>> {
    let path = source?.parent()?.join(file);
    if !path.exists() {
        return None;
    }
    Some(std::fs::read(&path).map_err(|err| err.to_string()))
}

/// What a [`COVER_SOURCE`] binding samples with no art. Near-black, so
/// sorting or dissolving the art degrades to nothing rather than a flash.
pub fn fallback_cover() -> AssetImage {
    const EDGE: usize = 8;
    AssetImage {
        width: EDGE as u32,
        height: EDGE as u32,
        rgba8: [26, 26, 26, 255].repeat(EDGE * EDGE),
    }
}

/// Straight alpha, which the window API takes.
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

/// Register a whole shader program with a window. Every shader surface
/// calls this.
pub fn register_program(
    window: &mut Window,
    source: &str,
    ctx: &ProgramCtx,
) -> Result<UserShaderId, String> {
    let spec = parse_chain(source)?;
    if spec.plain() {
        return window.register_user_shader(source);
    }
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

/// A copy of the window API's uniform block, so a check can compile without
/// registering. Keep it in sync by hand: registering per keystroke would
/// leave a compiled pipeline behind each time. Apply still goes through the
/// real registration, so the window's answer is always the final one.
const PARAMS_STRUCT: &str = "
struct ShaderParams {
    time: f32,
    delta: f32,
    resolution: vec2<f32>,
    mouse: vec4<f32>,
    user_meta: array<vec4<f32>, 2>,
    signals: array<vec4<f32>, 4>,
}
";

/// The screen-pass template. Same copy caveat as [`PARAMS_STRUCT`].
const SCREEN_TEMPLATE: &str = "
struct PostVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_post(@builtin(vertex_index) vertex_id: u32) -> PostVarying {
    let corner = vec2<f32>(f32((vertex_id << 1u) & 2u), f32(vertex_id & 2u));
    var out = PostVarying();
    out.position = vec4<f32>(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
    out.uv = corner;
    return out;
}

@fragment
fn fs_post(input: PostVarying) -> @location(0) vec4<f32> {
    return fs_user(input.uv);
}
";

struct Binding {
    name: String,
    declaration: &'static str,
    kind: &'static str,
}

impl Binding {
    fn texture(name: impl Into<String>) -> Binding {
        Binding {
            name: name.into(),
            declaration: "var ",
            kind: "texture_2d<f32>",
        }
    }
}

/// Everything a pass may bind, in the order registration pins. Offers the
/// superset like registration does; a later pass isn't in it, so a
/// reference to one is an unknown identifier.
fn pass_bindings(spec: &ChainSpec, index: usize) -> Vec<Binding> {
    let mut bindings = vec![
        Binding {
            name: "params".to_string(),
            declaration: "var<uniform> ",
            kind: "ShaderParams",
        },
        Binding::texture("screen"),
        Binding {
            name: "samp".to_string(),
            declaration: "var ",
            kind: "sampler",
        },
        Binding::texture("prev"),
        Binding::texture("mask"),
    ];
    bindings.extend(
        spec.passes[..index]
            .iter()
            .map(|pass| Binding::texture(pass.name.clone())),
    );
    bindings.extend(
        spec.assets
            .iter()
            .map(|asset| Binding::texture(asset.name.clone())),
    );
    bindings
}

/// One pass as a standalone module, with the explicit bind points naga
/// wants when nothing builds a pipeline layout.
fn compose_pass(user_source: &str, bindings: &[Binding]) -> String {
    let mut declarations = String::new();
    for (slot, binding) in bindings.iter().enumerate() {
        declarations.push_str(&format!(
            "@group(0) @binding({slot}) {}{}: {};\n",
            binding.declaration, binding.name, binding.kind
        ));
    }
    format!("{PARAMS_STRUCT}\n{SCREEN_TEMPLATE}\n{declarations}\n{user_source}")
}

/// [`register_program`] minus the pipeline, for the editor's
/// check-while-you-type. It can't tell that the backend has no shader
/// pipeline at all: that verdict only exists inside a window.
pub fn validate_program(source: &str, ctx: &ProgramCtx) -> Result<(), String> {
    let spec = parse_chain(source)?;
    // A missing plate reads out while typing rather than on apply.
    resolve_assets(&spec, ctx, None)?;
    for (index, pass) in spec.passes.iter().enumerate() {
        let bindings = pass_bindings(&spec, index);
        let composed = compose_pass(&pass.body, &bindings);
        let module =
            validate_wgsl(&composed).map_err(|err| format!("pass '{}': {err}", pass.name))?;
        // Module-scope variables validate, then break the renderer's
        // pipeline layout. Registration refuses them, so this does too.
        for variable in module.global_variables.iter() {
            let name = variable.1.name.as_deref().unwrap_or_default();
            if bindings.iter().any(|binding| binding.name == name) {
                continue;
            }
            let offered = bindings
                .iter()
                .map(|binding| binding.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "pass '{}': user shaders can't declare module-scope variables, found \
                 `{name}`; the template provides {offered}",
                pass.name
            ));
        }
    }
    Ok(())
}

/// Validate a hand-built frame pass, like the Milkdrop panel's and
/// backdrop's, whose dynamic textures are named in `textures`. Lets their
/// WGSL string constants get checked in a unit test.
pub fn validate_frame_pass(user_source: &str, textures: &[&str]) -> Result<(), String> {
    let mut bindings = vec![
        Binding {
            name: "params".to_string(),
            declaration: "var<uniform> ",
            kind: "ShaderParams",
        },
        Binding {
            name: "samp".to_string(),
            declaration: "var ",
            kind: "sampler",
        },
    ];
    bindings.extend(textures.iter().map(|name| Binding::texture(*name)));
    validate_wgsl(&compose_pass(user_source, &bindings)).map(|_| ())
}

fn validate_wgsl(source: &str) -> Result<naga::Module, String> {
    let module = naga::front::wgsl::parse_str(source).map_err(|err| err.emit_to_string(source))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|err| err.emit_to_string(source))?;
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_core::settings::{NamedShader, ShaderAsset};

    const FS_USER: &str = "fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";

    #[test]
    fn a_text_with_no_directives_is_one_pass() {
        let spec = parse_chain(FS_USER).expect("parse");
        assert_eq!(spec.passes.len(), 1);
        assert_eq!(spec.passes[0].name, "main");
        assert_eq!(spec.passes[0].body, FS_USER);
        assert_eq!(spec.passes[0].scale, 1.0);
        assert!(spec.assets.is_empty());
        assert!(spec.plain(), "and it takes the old registration path");

        let slotted = format!("// @slot 0: bass\n// @passing thought\n{FS_USER}");
        let spec = parse_chain(&slotted).expect("parse");
        assert_eq!(spec.passes.len(), 1);
        assert_eq!(spec.passes[0].body, slotted);
    }

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

        assert!(err("// @pass a: 0.3\n// @pass b\n").contains("isn't one of"));
        assert!(err("// @pass a: half\n// @pass b\n").contains("isn't a number"));
        assert!(err("// @pass a: 0.5\n").contains("has to be full size"));

        let many: String = (0..9).map(|n| format!("// @pass p{n}\n")).collect();
        assert!(err(&many).contains("capped at 8 passes"));
        let images: String = (0..9)
            .map(|n| format!("// @asset a{n}: {n}.png\n"))
            .collect();
        assert!(err(&images).contains("capped at 8 images"));

        assert!(err("// @asset plate: ../../secrets.png\n").contains("plain file name"));
        assert!(err("// @asset plate:\n").contains("needs a name and a file"));
    }

    #[test]
    fn a_program_gets_its_verdict_without_being_registered() {
        let detached = ProgramCtx::detached();
        validate_program(FS_USER, &detached).expect("a plain shader compiles");

        let chained = "\
// @pass down: 0.5
fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(screen, samp, uv) * params.time;
}
// @pass up
fn fs_user(uv: vec2<f32>) -> vec4<f32> { return textureSample(down, samp, uv); }";
        validate_program(chained, &detached).expect("a chain compiles");

        let broken = "fn fs_user(uv: vec2<f32>) -> vec4<f32> { return nope(uv); }";
        let err = validate_program(broken, &detached).expect_err("no such function");
        assert!(err.starts_with("pass 'main':"), "{err}");

        let ahead = "\
// @pass a
fn fs_user(uv: vec2<f32>) -> vec4<f32> { return textureSample(b, samp, uv); }
// @pass b
fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";
        assert!(validate_program(ahead, &detached).is_err());

        let global = "\
var<private> drift: f32;
fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(drift); }";
        let err = validate_program(global, &detached).expect_err("module-scope");
        assert!(err.contains("module-scope variables"), "{err}");

        let plate = format!("// @asset plate: plate.png\n{FS_USER}");
        let err = validate_program(&plate, &detached).expect_err("detached");
        assert!(err.contains("asset 'plate'"), "{err}");
    }

    fn plate() -> Vec<u8> {
        let mut image = image::RgbaImage::new(2, 2);
        image.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode");
        bytes.into_inner()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-shader-assets-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn an_image_resolves_from_the_pool_or_from_beside_the_source() {
        let source = format!("// @asset plate: plate.png\n{FS_USER}");
        let spec = parse_chain(&source).expect("parse");
        assert_eq!(spec.assets.len(), 1);
        assert_eq!(spec.assets[0].name, "plate");
        assert_eq!(spec.assets[0].file, "plate.png");

        let detached = resolve_assets(&spec, &ProgramCtx::detached(), None).expect_err("detached");
        assert!(detached.contains("asset 'plate'"), "{detached}");
        assert!(detached.contains("from a file"), "{detached}");

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

        let dir = scratch("resolve");
        let wgsl = dir.join("stamp.wgsl");
        std::fs::write(&wgsl, &source).expect("write");
        std::fs::write(dir.join("plate.png"), plate()).expect("write");
        let sibling =
            resolve_assets(&spec, &ProgramCtx::file(&wgsl), None).expect("from the folder");
        assert_eq!(sibling[0].1.width, 2);

        let empty = scratch("resolve-empty");
        let missing = resolve_assets(&spec, &ProgramCtx::file(empty.join("stamp.wgsl")), None)
            .expect_err("nothing there");
        assert!(missing.contains("plate.png isn't"), "{missing}");

        std::fs::write(dir.join("plate.png"), b"not a png").expect("write");
        let broken =
            resolve_assets(&spec, &ProgramCtx::file(&wgsl), None).expect_err("not an image");
        assert!(broken.starts_with("asset 'plate':"), "{broken}");

        rox_core::settings::note_shader_pool(Vec::new());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&empty).ok();
    }

    #[test]
    fn a_text_with_no_images_asks_nothing_of_its_context() {
        let spec = parse_chain(FS_USER).expect("parse");
        assert!(
            resolve_assets(&spec, &ProgramCtx::detached(), None)
                .expect("nothing to find")
                .is_empty()
        );
    }

    #[test]
    fn a_cover_binding_comes_from_the_player_not_a_folder() {
        let source = format!("// @asset art: @cover\n{FS_USER}");
        let spec = parse_chain(&source).expect("parse");
        assert!(spec.wants_cover());
        assert!(spec.assets[0].is_cover());
        assert!(uses_cover(&source));
        assert!(!uses_cover(FS_USER));

        let bound = resolve_assets(&spec, &ProgramCtx::detached(), None).expect("fallback");
        assert_eq!(bound.len(), 1);
        assert_eq!(bound[0].0, "art");
        assert_eq!(bound[0].1, fallback_cover());

        let art = decode(&plate()).expect("decode");
        let bound = resolve_assets(&spec, &ProgramCtx::detached(), Some(&art)).expect("cover");
        assert_eq!(bound[0].1, art);

        let both = format!("// @asset art: @cover\n// @asset plate: plate.png\n{FS_USER}");
        let spec = parse_chain(&both).expect("parse");
        let err = resolve_assets(&spec, &ProgramCtx::detached(), None).expect_err("detached");
        assert!(err.contains("asset 'plate'"), "{err}");

        let bogus =
            parse_chain(&format!("// @asset art: @screen\n{FS_USER}")).expect_err("not a source");
        assert!(bogus.contains("@cover"), "{bogus}");
    }
}
