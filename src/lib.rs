//! Rust bindings to NVIDIA Jetson's multimedia libraries: `NvBufSurface` dmabufs, VIC transforms
//! (`NvBufSurfTransform`) and the NVJPG hardware JPEG encoder (L4T's `libnvjpeg`).
//!
//! The libraries are opened at runtime, so this crate builds anywhere and [`Jetson::load`] reports what is missing.
//! The header layouts changed in JetPack 7, so the bindings for the running L4T release are picked at load time.
//!
//! ```no_run
//! use jetson_multimedia::{ColorFormat, Filter, Jetson, JpegEncoder, Surface, Tag};
//!
//! let jetson = Jetson::load()?;
//! let camera = Surface::new(&jetson, 1920, 1080, ColorFormat::Uyvy, Tag::Camera)?;
//! let yuv420 = Surface::new(&jetson, 1920, 1080, ColorFormat::Yuv420, Tag::None)?;
//! // ... have V4L2 fill `camera.fd()` as a DMABUF ...
//! jetson.transform(&camera, &yuv420, Filter::Nearest)?;
//! let mut encoder = JpegEncoder::new(&jetson, 1920, 1080)?;
//! let jpeg: &[u8] = encoder.encode(&yuv420, 90)?;
//! # Ok::<(), jetson_multimedia::Error>(())
//! ```

use std::ffi::c_void;
use std::fmt;
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(target_pointer_width = "64")]
pub mod sys;

#[cfg(target_pointer_width = "64")]
mod l4t_r35_r36 {
    use crate::sys::l4t_r35_r36 as sys;
    include!("backend.rs");
}

#[cfg(target_pointer_width = "64")]
mod l4t_r38 {
    use crate::sys::l4t_r38 as sys;
    include!("backend.rs");
}

const JPEG_LIB_VERSION: c_int = 80;
const JMSG_LENGTH_MAX: usize = 200;
const RELEASE_FILE: &str = "/etc/nv_tegra_release";
const LIBRARY_DIRS: [&str; 2] = [
    "/usr/lib/aarch64-linux-gnu/nvidia",
    "/usr/lib/aarch64-linux-gnu/tegra",
];

/// Everything that can go wrong here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A library or the release file could not be found or opened.
    Load(String),
    /// An L4T release this crate has no bindings for.
    UnsupportedRelease(u32),
    /// A call into NVIDIA's libraries failed.
    Call(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Load(message) | Error::Call(message) => f.write_str(message),
            Error::UnsupportedRelease(major) => {
                write!(f, "no bindings for L4T R{major} (supported: R35, R36, R38)")
            }
        }
    }
}

impl std::error::Error for Error {}

/// The L4T major release, which decides the struct layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// JetPack 5.
    R35,
    /// JetPack 6.
    R36,
    /// JetPack 7.
    R38,
}

impl Release {
    /// Reads `/etc/nv_tegra_release` ("# R36 (release), REVISION: 3.0, ...").
    pub fn detect() -> Result<Self, Error> {
        let text = std::fs::read_to_string(RELEASE_FILE)
            .map_err(|e| Error::Load(format!("{RELEASE_FILE}: {e} (not a Jetson?)")))?;
        Self::parse(&text)
    }

    fn parse(text: &str) -> Result<Self, Error> {
        let major = text
            .trim_start_matches(|c: char| c == '#' || c.is_whitespace())
            .strip_prefix('R')
            .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|digits| digits.parse::<u32>().ok())
            .ok_or_else(|| {
                Error::Load(format!(
                    "cannot read a release from {RELEASE_FILE}: {text:?}"
                ))
            })?;
        Self::from_major(major)
    }

    /// Maps an L4T major version (35, 36, 38) to its release.
    pub fn from_major(major: u32) -> Result<Self, Error> {
        match major {
            35 => Ok(Release::R35),
            36 => Ok(Release::R36),
            38 => Ok(Release::R38),
            other => Err(Error::UnsupportedRelease(other)),
        }
    }
}

/// Pixel formats with the same value in every supported release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ColorFormat {
    Gray8 = 1,
    Yuv420 = 2,
    Nv12 = 6,
    Uyvy = 10,
    Vyuy = 12,
    Yuyv = 14,
    Yvyu = 16,
    Rgba = 19,
    Bgra = 20,
}

/// What the buffer is for; the driver uses it to pick the memory pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Tag {
    None = 0,
    Camera = 0x200,
    VideoConvert = 0xf01,
    VideoEncode = 0x1200,
    VideoDecode = 0x1400,
    Jpeg = 0x1500,
}

/// Scaling filter for [`Jetson::transform`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Filter {
    Nearest = 0,
    Bilinear = 1,
    /// NVIDIA's 5-tap filter.
    FiveTap = 2,
    /// NVIDIA's 10-tap filter.
    TenTap = 3,
    Smart = 4,
    Nicest = 5,
    Default = 6,
}

/// A pixel rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn full(width: u32, height: u32) -> Self {
        Rect {
            top: 0,
            left: 0,
            width,
            height,
        }
    }
}

/// Plane 0 of a surface, as the driver allocated it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceInfo {
    /// The dmabuf, e.g. for `V4L2_MEMORY_DMABUF`.
    pub fd: c_int,
    pub width: u32,
    pub height: u32,
    /// Bytes per row of plane 0.
    pub pitch: u32,
    /// The raw `NvBufSurfaceColorFormat`.
    pub color_format: u32,
    /// Bytes across all planes.
    pub data_size: u32,
}

trait Backend: Send + Sync {
    fn allocate(
        &self,
        width: u32,
        height: u32,
        format: u32,
        tag: u32,
    ) -> Result<*mut c_void, Error>;
    fn wrap_fd(&self, fd: c_int) -> Result<*mut c_void, Error>;
    fn destroy(&self, surface: *mut c_void);
    fn info(&self, surface: *mut c_void) -> SurfaceInfo;
    fn transform(
        &self,
        source: *mut c_void,
        target: *mut c_void,
        source_rect: Rect,
        target_rect: Rect,
        filter: Filter,
    ) -> Result<(), Error>;
    fn encoder(&self, capacity: usize) -> Result<Box<dyn EncoderBackend>, Error>;
}

trait EncoderBackend: Send {
    fn encode(&mut self, fd: c_int, quality: c_int) -> Result<&[u8], Error>;
}

/// The loaded libraries. Cheap to clone.
#[derive(Clone)]
pub struct Jetson {
    backend: Arc<dyn Backend>,
    release: Release,
}

impl fmt::Debug for Jetson {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Jetson")
            .field("release", &self.release)
            .finish()
    }
}

impl Jetson {
    /// Detects the release and loads the libraries from L4T's library directory.
    pub fn load() -> Result<Self, Error> {
        Self::load_release(Release::detect()?)
    }

    /// Loads the libraries with the bindings for `release`, from `$JETSON_MULTIMEDIA_LIB_DIR` or L4T's library directory.
    pub fn load_release(release: Release) -> Result<Self, Error> {
        let dir = match std::env::var_os("JETSON_MULTIMEDIA_LIB_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => LIBRARY_DIRS
                .iter()
                .map(PathBuf::from)
                .find(|dir| dir.is_dir())
                .ok_or_else(|| {
                    Error::Load(format!("none of {LIBRARY_DIRS:?} exist (not a Jetson?)"))
                })?,
        };
        Self::load_from(release, &dir)
    }

    /// Loads the libraries from `dir` with the bindings for `release`.
    pub fn load_from(release: Release, dir: &Path) -> Result<Self, Error> {
        #[cfg(target_pointer_width = "64")]
        {
            let backend: Arc<dyn Backend> = match release {
                Release::R35 | Release::R36 => Arc::new(l4t_r35_r36::Libraries::load(dir)?),
                Release::R38 => Arc::new(l4t_r38::Libraries::load(dir)?),
            };
            Ok(Jetson { backend, release })
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            let _ = dir;
            Err(Error::UnsupportedRelease(release as u32))
        }
    }

    pub fn release(&self) -> Release {
        self.release
    }

    /// Converts and scales all of `source` into all of `target` on the VIC.
    pub fn transform(
        &self,
        source: &Surface,
        target: &Surface,
        filter: Filter,
    ) -> Result<(), Error> {
        let (s, t) = (source.info(), target.info());
        self.transform_rect(
            source,
            target,
            Rect::full(s.width, s.height),
            Rect::full(t.width, t.height),
            filter,
        )
    }

    /// Converts and scales `source_rect` of `source` into `target_rect` of `target` on the VIC.
    pub fn transform_rect(
        &self,
        source: &Surface,
        target: &Surface,
        source_rect: Rect,
        target_rect: Rect,
        filter: Filter,
    ) -> Result<(), Error> {
        self.backend
            .transform(source.raw, target.raw, source_rect, target_rect, filter)
    }
}

/// One `NvBufSurface`. Destroyed on drop when allocated here; a surface from [`Surface::from_fd`] is only borrowed.
pub struct Surface {
    backend: Arc<dyn Backend>,
    raw: *mut c_void,
    owned: bool,
}

// SAFETY: an NvBufSurface is a handle to driver memory with no thread affinity.
unsafe impl Send for Surface {}
// SAFETY: every method takes &self and only reads the surface's metadata or hands it to the driver.
unsafe impl Sync for Surface {}

impl Surface {
    /// Allocates a pitch-linear surface.
    pub fn new(
        jetson: &Jetson,
        width: u32,
        height: u32,
        format: ColorFormat,
        tag: Tag,
    ) -> Result<Self, Error> {
        let raw = jetson
            .backend
            .allocate(width, height, format as u32, tag as u32)?;
        Ok(Surface {
            backend: jetson.backend.clone(),
            raw,
            owned: true,
        })
    }

    /// Wraps a dmabuf that is already an `NvBufSurface`, without taking ownership.
    pub fn from_fd(jetson: &Jetson, fd: c_int) -> Result<Self, Error> {
        let raw = jetson.backend.wrap_fd(fd)?;
        Ok(Surface {
            backend: jetson.backend.clone(),
            raw,
            owned: false,
        })
    }

    pub fn info(&self) -> SurfaceInfo {
        self.backend.info(self.raw)
    }

    /// The dmabuf behind the surface, valid while it lives.
    pub fn fd(&self) -> c_int {
        self.info().fd
    }

    /// The `NvBufSurface*`, for calls this crate does not wrap.
    pub fn as_raw(&self) -> *mut c_void {
        self.raw
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        if self.owned {
            self.backend.destroy(self.raw);
        }
    }
}

impl fmt::Debug for Surface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Surface")
            .field("info", &self.info())
            .field("owned", &self.owned)
            .finish()
    }
}

/// The NVJPG hardware JPEG encoder. Takes YUV420 surfaces.
///
/// A fatal libjpeg error still ends the process through libjpeg's default `error_exit`, as it would in C;
/// `jpeg_start_compress` failures are caught and returned.
pub struct JpegEncoder {
    inner: Box<dyn EncoderBackend>,
    _backend: Arc<dyn Backend>,
}

impl JpegEncoder {
    /// `width` and `height` size the initial output buffer (grown when a frame needs more).
    pub fn new(jetson: &Jetson, width: u32, height: u32) -> Result<Self, Error> {
        let capacity = (width as usize * height as usize * 3 / 2).max(4096);
        Ok(JpegEncoder {
            inner: jetson.backend.encoder(capacity)?,
            _backend: jetson.backend.clone(),
        })
    }

    /// Encodes `source` (YUV420) at `quality` 1-100. The bytes are valid until the next call.
    pub fn encode(&mut self, source: &Surface, quality: u8) -> Result<&[u8], Error> {
        self.inner
            .encode(source.fd(), quality.clamp(1, 100) as c_int)
    }
}

impl fmt::Debug for JpegEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JpegEncoder").finish_non_exhaustive()
    }
}

fn find(dir: &Path, name: &str) -> Result<PathBuf, Error> {
    for file in [
        format!("{name}.so.1.0.0"),
        format!("{name}.so.1"),
        format!("{name}.so"),
    ] {
        let path = dir.join(file);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(Error::Load(format!(
        "{name}.so not found in {}",
        dir.display()
    )))
}

// Never unloaded: libnvjpeg registers thread-exit destructors that would otherwise point at unmapped code.
// RTLD_LOCAL keeps libnvjpeg's vendor-extended jpeg_* symbols away from any stock libjpeg in the process.
fn open(dir: &Path, name: &str) -> Result<libloading::Library, Error> {
    let path = find(dir, name)?;
    #[cfg(unix)]
    {
        use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};
        const RTLD_NODELETE: std::os::raw::c_int = 0x1000;
        // SAFETY: NVIDIA's own libraries; their initialisers have no preconditions.
        unsafe { Library::open(Some(&path), RTLD_NOW | RTLD_LOCAL | RTLD_NODELETE) }
            .map(Into::into)
            .map_err(|e| Error::Load(format!("{}: {e}", path.display())))
    }
    #[cfg(not(unix))]
    {
        Err(Error::Load(format!("{} needs Linux", path.display())))
    }
}

extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(pointer: *mut c_void);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_release_file() {
        let text = "# R36 (release), REVISION: 3.0, GCID: 36191598, BOARD: generic, EABI: aarch64, DATE: Mon May  6 2024\n";
        assert_eq!(Release::parse(text), Ok(Release::R36));
        assert_eq!(
            Release::parse("# R35 (release), REVISION: 6.4"),
            Ok(Release::R35)
        );
        assert_eq!(
            Release::parse("# R38 (release), REVISION: 2.2"),
            Ok(Release::R38)
        );
        assert_eq!(
            Release::parse("# R32 (release), REVISION: 7.1"),
            Err(Error::UnsupportedRelease(32))
        );
        assert!(Release::parse("garbage").is_err());
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn enum_values_match_every_release() {
        macro_rules! check {
            ($sys:ident) => {{
                use crate::sys::$sys as s;
                let formats = [
                    (
                        ColorFormat::Gray8,
                        s::NvBufSurfaceColorFormat_NVBUF_COLOR_FORMAT_GRAY8,
                    ),
                    (
                        ColorFormat::Yuv420,
                        s::NvBufSurfaceColorFormat_NVBUF_COLOR_FORMAT_YUV420,
                    ),
                    (
                        ColorFormat::Nv12,
                        s::NvBufSurfaceColorFormat_NVBUF_COLOR_FORMAT_NV12,
                    ),
                    (
                        ColorFormat::Uyvy,
                        s::NvBufSurfaceColorFormat_NVBUF_COLOR_FORMAT_UYVY,
                    ),
                    (
                        ColorFormat::Vyuy,
                        s::NvBufSurfaceColorFormat_NVBUF_COLOR_FORMAT_VYUY,
                    ),
                    (
                        ColorFormat::Yuyv,
                        s::NvBufSurfaceColorFormat_NVBUF_COLOR_FORMAT_YUYV,
                    ),
                    (
                        ColorFormat::Yvyu,
                        s::NvBufSurfaceColorFormat_NVBUF_COLOR_FORMAT_YVYU,
                    ),
                    (
                        ColorFormat::Rgba,
                        s::NvBufSurfaceColorFormat_NVBUF_COLOR_FORMAT_RGBA,
                    ),
                    (
                        ColorFormat::Bgra,
                        s::NvBufSurfaceColorFormat_NVBUF_COLOR_FORMAT_BGRA,
                    ),
                ];
                for (ours, theirs) in formats {
                    assert_eq!(
                        ours as u32,
                        theirs as u32,
                        "{ours:?} in {}",
                        stringify!($sys)
                    );
                }
                let tags = [
                    (Tag::None, s::NvBufSurfaceTag_NvBufSurfaceTag_NONE),
                    (Tag::Camera, s::NvBufSurfaceTag_NvBufSurfaceTag_CAMERA),
                    (
                        Tag::VideoConvert,
                        s::NvBufSurfaceTag_NvBufSurfaceTag_VIDEO_CONVERT,
                    ),
                    (
                        Tag::VideoEncode,
                        s::NvBufSurfaceTag_NvBufSurfaceTag_VIDEO_ENC,
                    ),
                    (
                        Tag::VideoDecode,
                        s::NvBufSurfaceTag_NvBufSurfaceTag_VIDEO_DEC,
                    ),
                    (Tag::Jpeg, s::NvBufSurfaceTag_NvBufSurfaceTag_JPEG),
                ];
                for (ours, theirs) in tags {
                    assert_eq!(
                        ours as u32,
                        theirs as u32,
                        "{ours:?} in {}",
                        stringify!($sys)
                    );
                }
                let filters = [
                    (
                        Filter::Nearest,
                        s::NvBufSurfTransform_Inter_NvBufSurfTransformInter_Nearest,
                    ),
                    (
                        Filter::Bilinear,
                        s::NvBufSurfTransform_Inter_NvBufSurfTransformInter_Bilinear,
                    ),
                    (
                        Filter::FiveTap,
                        s::NvBufSurfTransform_Inter_NvBufSurfTransformInter_Algo1,
                    ),
                    (
                        Filter::TenTap,
                        s::NvBufSurfTransform_Inter_NvBufSurfTransformInter_Algo2,
                    ),
                    (
                        Filter::Smart,
                        s::NvBufSurfTransform_Inter_NvBufSurfTransformInter_Algo3,
                    ),
                    (
                        Filter::Nicest,
                        s::NvBufSurfTransform_Inter_NvBufSurfTransformInter_Algo4,
                    ),
                    (
                        Filter::Default,
                        s::NvBufSurfTransform_Inter_NvBufSurfTransformInter_Default,
                    ),
                ];
                for (ours, theirs) in filters {
                    assert_eq!(
                        ours as u32,
                        theirs as u32,
                        "{ours:?} in {}",
                        stringify!($sys)
                    );
                }
            }};
        }
        check!(l4t_r35_r36);
        check!(l4t_r38);
    }

    #[test]
    fn load_fails_cleanly_off_a_jetson() {
        if Path::new(RELEASE_FILE).exists() {
            return;
        }
        assert!(matches!(Jetson::load(), Err(Error::Load(_))));
    }
}
