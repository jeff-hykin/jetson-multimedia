// Release-independent code, `include!`d once per binding module; `sys` names that module.

use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_ulong};
use std::path::Path;
use std::sync::Arc;

use crate::{Backend, EncoderBackend, Error, Filter, Rect, SurfaceInfo};

pub(crate) struct Libraries {
    surface: sys::NvBufSurfaceLib,
    transform: sys::NvBufSurfTransformLib,
    jpeg: Option<Arc<sys::NvJpegLib>>,
}

impl Libraries {
    pub(crate) fn load(dir: &Path) -> Result<Self, Error> {
        // SAFETY: these are NVIDIA's own libraries, loaded with the signatures generated from their headers.
        unsafe {
            Ok(Self {
                surface: sys::NvBufSurfaceLib::from_library(crate::open(dir, "libnvbufsurface")?)
                    .map_err(|e| Error::Load(format!("libnvbufsurface: {e}")))?,
                transform: sys::NvBufSurfTransformLib::from_library(crate::open(dir, "libnvbufsurftransform")?)
                    .map_err(|e| Error::Load(format!("libnvbufsurftransform: {e}")))?,
                // Optional: buffers and transforms work without the JPEG encoder.
                jpeg: crate::open(dir, "libnvjpeg").ok().and_then(|library| sys::NvJpegLib::from_library(library).ok()).map(Arc::new),
            })
        }
    }
}

impl Backend for Libraries {
    fn allocate(&self, width: u32, height: u32, format: u32, tag: u32) -> Result<*mut c_void, Error> {
        // SAFETY: zeroed is the documented initial state of the params; the pointer is checked before use.
        unsafe {
            let mut params: sys::NvBufSurfaceAllocateParams = std::mem::zeroed();
            params.params.width = width;
            params.params.height = height;
            params.params.memType = sys::NvBufSurfaceMemType_NVBUF_MEM_SURFACE_ARRAY;
            params.params.layout = sys::NvBufSurfaceLayout_NVBUF_LAYOUT_PITCH;
            params.params.colorFormat = format as _;
            params.memtag = tag as _;
            let mut surface: *mut sys::NvBufSurface = std::ptr::null_mut();
            if self.surface.NvBufSurfaceAllocate(&mut surface, 1, &mut params) != 0 || surface.is_null() {
                return Err(Error::Call(format!("NvBufSurfaceAllocate {width}x{height} format {format} failed")));
            }
            (*surface).numFilled = 1;
            Ok(surface.cast())
        }
    }

    fn wrap_fd(&self, fd: c_int) -> Result<*mut c_void, Error> {
        let mut surface: *mut c_void = std::ptr::null_mut();
        // SAFETY: NvBufSurfaceFromFd only writes the out pointer.
        let status = unsafe { self.surface.NvBufSurfaceFromFd(fd, &mut surface) };
        if status != 0 || surface.is_null() {
            return Err(Error::Call(format!("NvBufSurfaceFromFd({fd}) failed")));
        }
        Ok(surface)
    }

    fn destroy(&self, surface: *mut c_void) {
        // SAFETY: only called once, by the Surface that allocated it.
        unsafe {
            self.surface.NvBufSurfaceDestroy(surface.cast());
        }
    }

    fn info(&self, surface: *mut c_void) -> SurfaceInfo {
        // SAFETY: a live surface of batch size 1 has one entry in surfaceList.
        unsafe {
            let params = &*(*surface.cast::<sys::NvBufSurface>()).surfaceList;
            SurfaceInfo {
                fd: params.bufferDesc as c_int,
                width: params.width,
                height: params.height,
                pitch: params.pitch,
                color_format: params.colorFormat,
                data_size: params.dataSize,
            }
        }
    }

    fn transform(
        &self,
        source: *mut c_void,
        target: *mut c_void,
        source_rect: Rect,
        target_rect: Rect,
        filter: Filter,
    ) -> Result<(), Error> {
        // SAFETY: both surfaces are live; the rects outlive the call.
        unsafe {
            let mut source_rect = rect(source_rect);
            let mut target_rect = rect(target_rect);
            let mut params: sys::NvBufSurfTransformParams = std::mem::zeroed();
            params.transform_flag = sys::NvBufSurfTransform_Transform_Flag_NVBUFSURF_TRANSFORM_FILTER
                | sys::NvBufSurfTransform_Transform_Flag_NVBUFSURF_TRANSFORM_CROP_SRC
                | sys::NvBufSurfTransform_Transform_Flag_NVBUFSURF_TRANSFORM_CROP_DST;
            params.transform_filter = filter as _;
            params.src_rect = &mut source_rect;
            params.dst_rect = &mut target_rect;
            let status = self.transform.NvBufSurfTransform(source.cast(), target.cast(), &mut params);
            if status != sys::NvBufSurfTransform_Error_NvBufSurfTransformError_Success {
                return Err(Error::Call(format!("NvBufSurfTransform failed ({status})")));
            }
        }
        Ok(())
    }

    fn encoder(&self, capacity: usize) -> Result<Box<dyn EncoderBackend>, Error> {
        let jpeg = self.jpeg.clone().ok_or_else(|| Error::Load("libnvjpeg not found".into()))?;
        // SAFETY: cinfo and jerr are boxed so the pointers libjpeg keeps into them stay valid; the buffer is malloc'd
        // because jpeg_mem_dest may free and replace it.
        unsafe {
            let mut encoder = Box::new(Encoder {
                jpeg: jpeg.clone(),
                cinfo: std::mem::zeroed(),
                jerr: std::mem::zeroed(),
                buffer: crate::malloc(capacity).cast(),
                capacity: capacity as c_ulong,
            });
            if encoder.buffer.is_null() {
                return Err(Error::Call("out of memory".into()));
            }
            encoder.cinfo.err = jpeg.jpeg_std_error(&mut encoder.jerr);
            jpeg.jpeg_CreateCompress(
                &mut encoder.cinfo,
                crate::JPEG_LIB_VERSION,
                std::mem::size_of::<sys::jpeg_compress_struct>(),
            );
            jpeg.jpeg_suppress_tables(&mut encoder.cinfo, 1);
            Ok(encoder)
        }
    }
}

fn rect(rect: Rect) -> sys::NvBufSurfTransformRect {
    // SAFETY: plain data; zero fills R38's reserved words.
    let mut out: sys::NvBufSurfTransformRect = unsafe { std::mem::zeroed() };
    out.top = rect.top;
    out.left = rect.left;
    out.width = rect.width;
    out.height = rect.height;
    out
}

struct Encoder {
    jpeg: Arc<sys::NvJpegLib>,
    cinfo: sys::jpeg_compress_struct,
    jerr: sys::jpeg_error_mgr,
    buffer: *mut u8,
    capacity: c_ulong,
}

// SAFETY: the encoder's pointers all point into memory it owns; it is used from one thread at a time (&mut self).
unsafe impl Send for Encoder {}

impl EncoderBackend for Encoder {
    fn encode(&mut self, fd: c_int, quality: c_int) -> Result<&[u8], Error> {
        // SAFETY: fd is a live YUV420 NvBufSurface; libjpeg writes into `buffer`, replacing it if it outgrows it.
        unsafe {
            let jpeg = &*self.jpeg;
            let cinfo = &mut self.cinfo;
            let mut size = self.capacity;
            let before = self.buffer;
            jpeg.jpeg_mem_dest(cinfo, &mut self.buffer, &mut size);
            cinfo.fd = fd;
            cinfo.IsVendorbuf = 1;
            cinfo.raw_data_in = 1;
            cinfo.in_color_space = sys::J_COLOR_SPACE_JCS_YCbCr;
            jpeg.jpeg_set_defaults(cinfo);
            jpeg.jpeg_set_quality(cinfo, quality, 1);
            jpeg.jpeg_set_hardware_acceleration_parameters_enc(cinfo, 1, size as _, 0, 0);
            cinfo.in_color_space = sys::J_COLOR_SPACE_JCS_YCbCr;
            (*cinfo.err).msg_code = 0;
            jpeg.jpeg_start_compress(cinfo, 0);
            if (*cinfo.err).msg_code != 0 {
                let mut message = [0 as c_char; crate::JMSG_LENGTH_MAX];
                if let Some(format_message) = (*cinfo.err).format_message {
                    format_message((cinfo as *mut sys::jpeg_compress_struct).cast(), message.as_mut_ptr());
                }
                let message = std::ffi::CStr::from_ptr(message.as_ptr()).to_string_lossy().into_owned();
                return Err(Error::Call(format!("jpeg_start_compress: {message}")));
            }
            jpeg.jpeg_write_raw_data(cinfo, std::ptr::null_mut(), 0);
            jpeg.jpeg_finish_compress(cinfo);
            if self.buffer != before {
                crate::free(before.cast());
            }
            self.capacity = self.capacity.max(size);
            Ok(std::slice::from_raw_parts(self.buffer, size as usize))
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by jpeg_CreateCompress; buffer is the malloc'd one libjpeg last handed back.
        unsafe {
            self.jpeg.jpeg_destroy_compress(&mut self.cinfo);
            crate::free(self.buffer.cast());
        }
    }
}
