//! Windows Excel "Copy as Picture" metafile fallback.
//! The clipboard can offer CF_METAFILEPICT (WMF) or CF_ENHMETAFILE (EMF).
//! Windows synthesizes CF_ENHMETAFILE from CF_METAFILEPICT. This module is
//! called only for Excel image-only copies after the ordinary text/BIFF guard.
//! The borrowed clipboard object is duplicated once, the clipboard is closed,
//! and all GDI rasterization and PNG encoding happen outside the clipboard lock.

use std::{io::Cursor, mem, ptr, slice};

use clipboard_win::Clipboard as WindowsClipboard;
use image::{DynamicImage, ImageFormat, RgbaImage};
use winapi::shared::windef::{HBITMAP, HDC, HENHMETAFILE, HGDIOBJ, RECT};
use winapi::um::wingdi::{
    CopyEnhMetaFileW, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteEnhMetaFile,
    DeleteObject, GetEnhMetaFileHeader, GdiFlush, PlayEnhMetaFile, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, ENHMETAHEADER,
};
use winapi::um::winuser::{
    GetClipboardData, IsClipboardFormatAvailable, CF_ENHMETAFILE, CF_METAFILEPICT,
};

use super::payload::ImagePayload;
use crate::core::{AppError, Result};

const MAX_EMF_BYTES: u32 = 64 * 1024 * 1024;
const MAX_RASTER_EDGE: f64 = 8192.0;
const MAX_RASTER_PIXELS: f64 = 12_000_000.0;

/// Format availability is metadata only; never calls GetClipboardData.
/// WMF can be requested as synthesized EMF through the Windows clipboard.
pub fn is_advertised() -> bool {
    unsafe {
        IsClipboardFormatAvailable(CF_ENHMETAFILE) != 0
            || IsClipboardFormatAvailable(CF_METAFILEPICT) != 0
    }
}

struct OwnedEmf(HENHMETAFILE);

impl Drop for OwnedEmf {
    fn drop(&mut self) {
        unsafe { DeleteEnhMetaFile(self.0) };
    }
}

/// Acquire only an independently owned EMF handle while the clipboard is open.
/// The returned object is valid after CloseClipboard; no GDI rendering occurs here.
fn snapshot_emf() -> Result<OwnedEmf> {
    let _access = super::lock_windows_capture()?;
    let _clipboard = WindowsClipboard::new().map_err(|err| {
        AppError::Clipboard(format!("Excel metafile clipboard busy: {err}"))
    })?;
    if super::windows_write_pending() {
        return Err(AppError::Clipboard(
            "Windows clipboard busy: EcoPaste write pending".to_owned(),
        ));
    }
    log::info!("clipboard native_read request=CF_ENHMETAFILE owner_hint=Excel");
    unsafe {
        let borrowed = GetClipboardData(CF_ENHMETAFILE) as HENHMETAFILE;
        if borrowed.is_null() {
            return Err(AppError::Clipboard(
                "Excel WMF/EMF clipboard conversion unavailable".to_owned(),
            ));
        }
        let mut header: ENHMETAHEADER = mem::zeroed();
        if GetEnhMetaFileHeader(
            borrowed,
            mem::size_of::<ENHMETAHEADER>() as u32,
            &mut header,
        ) == 0
        {
            return Err(AppError::Clipboard(
                "Excel metafile header unavailable".to_owned(),
            ));
        }
        // Prevent arbitrarily large clipboard objects from exhausting memory.
        if header.nBytes == 0 || header.nBytes > MAX_EMF_BYTES {
            return Err(AppError::Clipboard(
                "Excel metafile exceeds supported memory budget".to_owned(),
            ));
        }
        let independent = CopyEnhMetaFileW(borrowed, ptr::null());
        if independent.is_null() {
            return Err(AppError::Clipboard(
                "Unable to copy Excel enhanced metafile".to_owned(),
            ));
        }
        Ok(OwnedEmf(independent))
    }
}

fn raster_dimensions(header: &ENHMETAHEADER) -> Result<(u32, u32)> {
    // rclFrame uses 0.01 mm; 96 dpi is the output's nominal raster density.
    let frame_width = (i64::from(header.rclFrame.right)
        - i64::from(header.rclFrame.left))
    .abs();
    let frame_height = (i64::from(header.rclFrame.bottom)
        - i64::from(header.rclFrame.top))
    .abs();
    let (width, height) = if frame_width > 0 && frame_height > 0 {
        (
            frame_width as f64 * 96.0 / 2540.0,
            frame_height as f64 * 96.0 / 2540.0,
        )
    } else {
        (
            (i64::from(header.rclBounds.right)
                - i64::from(header.rclBounds.left))
            .abs() as f64,
            (i64::from(header.rclBounds.bottom)
                - i64::from(header.rclBounds.top))
            .abs() as f64,
        )
    };
    if width < 1.0 || height < 1.0 {
        return Err(AppError::Clipboard(
            "Excel metafile has invalid picture dimensions".to_owned(),
        ));
    }
    // Limit raster resource use, not the user's ordinary Excel clipboard size.
    let scale = (MAX_RASTER_EDGE / width)
        .min(MAX_RASTER_EDGE / height)
        .min((MAX_RASTER_PIXELS / (width * height)).sqrt())
        .min(1.0);
    Ok((
        (width * scale).round().max(1.0) as u32,
        (height * scale).round().max(1.0) as u32,
    ))
}

struct DibSurface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: *mut u8,
}

impl DibSurface {
    fn new(width: u32, height: u32) -> Result<Self> {
        unsafe {
            let dc = CreateCompatibleDC(ptr::null_mut());
            if dc.is_null() {
                return Err(AppError::Clipboard(
                    "Cannot create Excel metafile raster context".to_owned(),
                ));
            }
            let mut info: BITMAPINFO = mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32), // top-down BGRA
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let mut raw = ptr::null_mut();
            let bitmap = CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut raw,
                ptr::null_mut(),
                0,
            );
            if bitmap.is_null() || raw.is_null() {
                if !bitmap.is_null() {
                    DeleteObject(bitmap as HGDIOBJ);
                }
                DeleteDC(dc);
                return Err(AppError::Clipboard(
                    "Cannot allocate Excel metafile raster".to_owned(),
                ));
            }
            let previous = SelectObject(dc, bitmap as HGDIOBJ);
            if previous.is_null() || previous as isize == -1 {
                DeleteObject(bitmap as HGDIOBJ);
                DeleteDC(dc);
                return Err(AppError::Clipboard(
                    "Cannot select Excel metafile bitmap".to_owned(),
                ));
            }
            Ok(Self {
                dc,
                bitmap,
                previous,
                pixels: raw as *mut u8,
            })
        }
    }
}

impl Drop for DibSurface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap as HGDIOBJ);
            DeleteDC(self.dc);
        }
    }
}

/// Rasterize a copied EMF on a bounded top-down 32-bit DIB with white backing.
fn rasterize(emf: &OwnedEmf, width: u32, height: u32) -> Result<Vec<u8>> {
    let surface = DibSurface::new(width, height)?;
    let byte_count = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| AppError::Clipboard("Excel picture size overflow".to_owned()))?;
    unsafe {
        slice::from_raw_parts_mut(surface.pixels, byte_count).fill(255);
        let bounds = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        if PlayEnhMetaFile(surface.dc, emf.0, &bounds) == 0 {
            return Err(AppError::Clipboard(
                "Excel metafile rasterization failed".to_owned(),
            ));
        }
        GdiFlush();
        let bgra = slice::from_raw_parts(surface.pixels, byte_count);
        let mut rgba = Vec::with_capacity(byte_count);
        for pixel in bgra.chunks_exact(4) {
            // GDI may not write the alpha channel; use an opaque background.
            rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
        Ok(rgba)
    }
}

/// Return PNG bytes for the existing ImagePayload -> ImageStore pipeline.
pub fn capture_png() -> Result<Option<ImagePayload>> {
    if !is_advertised() {
        return Ok(None);
    }
    let emf = snapshot_emf()?;
    let mut header: ENHMETAHEADER = unsafe { mem::zeroed() };
    if unsafe {
        GetEnhMetaFileHeader(
            emf.0,
            mem::size_of::<ENHMETAHEADER>() as u32,
            &mut header,
        )
    } == 0
    {
        return Err(AppError::Clipboard(
            "Copied Excel metafile header unavailable".to_owned(),
        ));
    }
    let (width, height) = raster_dimensions(&header)?;
    let rgba = rasterize(&emf, width, height)?;
    let picture = RgbaImage::from_raw(width, height, rgba).ok_or_else(|| {
        AppError::Clipboard("Invalid Excel metafile raster buffer".to_owned())
    })?;
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(picture)
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|err| AppError::Clipboard(format!("Excel metafile PNG encoding: {err}")))?;
    log::info!("clipboard excel vector_image=png width={width} height={height}");
    Ok(Some(ImagePayload {
        bytes: output.into_inner(),
        width,
        height,
    }))
}
