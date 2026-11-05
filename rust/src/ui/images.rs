//! Pictures with WIC (Windows Imaging Component, built into Windows): reading tile pictures at
//! exactly the size they're shown, and saving a clip's picture as a 256x256 PNG cropped to a
//! square (`ImageOps.fit` in the Python app), with the photo's EXIF rotation applied.
//!
//! Tile pictures are decoded on a background thread, so a group full of pictures never holds up
//! a frame; the UI thread only turns finished pixels into a Direct2D bitmap.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use windows::core::{GUID, HSTRING};
use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::VT_UI2;

pub const PICTURE_SIZE: u32 = 256;

pub fn factory() -> windows::core::Result<IWICImagingFactory> {
    // SAFETY: COM is initialised on the calling thread.
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

/// EXIF orientation (1-8) -> WIC transform.
fn orientation(f: &IWICBitmapFrameDecode) -> WICBitmapTransformOptions {
    // SAFETY: metadata query on a valid frame; the PROPVARIANT is cleared after reading.
    let o = unsafe {
        let Ok(r) = f.GetMetadataQueryReader() else { return WICBitmapTransformRotate0 };
        let mut v = PROPVARIANT::default();
        let mut o = 1u16;
        for q in ["/app1/ifd/{ushort=274}", "/ifd/{ushort=274}"] {
            if r.GetMetadataByName(&HSTRING::from(q), &mut v).is_ok() {
                if v.Anonymous.Anonymous.vt == VT_UI2 {
                    o = v.Anonymous.Anonymous.Anonymous.uiVal;
                }
                let _ = PropVariantClear(&mut v);
                break;
            }
        }
        o
    };
    match o {
        2 => WICBitmapTransformFlipHorizontal,
        3 => WICBitmapTransformRotate180,
        4 => WICBitmapTransformFlipVertical,
        5 => WICBitmapTransformOptions(WICBitmapTransformRotate90.0 | WICBitmapTransformFlipHorizontal.0),
        6 => WICBitmapTransformRotate90,
        7 => WICBitmapTransformOptions(WICBitmapTransformRotate270.0 | WICBitmapTransformFlipHorizontal.0),
        8 => WICBitmapTransformRotate270,
        _ => WICBitmapTransformRotate0,
    }
}

/// Centre-crop `src` to a square and scale it to size x size, in `format`.
fn fit_square(wic: &IWICImagingFactory, src: &IWICBitmapSource, size: u32, format: &GUID) -> windows::core::Result<IWICBitmapSource> {
    // SAFETY: WIC pipeline objects on valid sources.
    unsafe {
        let (mut w, mut h) = (0, 0);
        src.GetSize(&mut w, &mut h)?;
        let side = w.min(h).max(1);
        let rc = WICRect { X: ((w - side) / 2) as i32, Y: ((h - side) / 2) as i32, Width: side as i32, Height: side as i32 };
        let clip = wic.CreateBitmapClipper()?;
        clip.Initialize(src, &rc)?;
        let conv = wic.CreateFormatConverter()?;
        conv.Initialize(&clip, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let scaled: IWICBitmapSource = if side == size {
            conv.into()
        } else {
            let s = wic.CreateBitmapScaler()?;
            // Fant when shrinking (box-like, no ringing), cubic when growing
            let mode = if side > size { WICBitmapInterpolationModeFant } else { WICBitmapInterpolationModeHighQualityCubic };
            s.Initialize(&conv, size, size, mode)?;
            s.into()
        };
        if *format == GUID_WICPixelFormat32bppPBGRA {
            return Ok(scaled);
        }
        let out = wic.CreateFormatConverter()?;
        out.Initialize(&scaled, format, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        Ok(out.into())
    }
}

fn open_oriented(wic: &IWICImagingFactory, path: &Path) -> windows::core::Result<IWICBitmapSource> {
    // SAFETY: WIC decode of a file path.
    unsafe {
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(path.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        let frame = dec.GetFrame(0)?;
        let t = orientation(&frame);
        if t == WICBitmapTransformRotate0 {
            return Ok(frame.into());
        }
        let rot = wic.CreateBitmapFlipRotator()?;
        rot.Initialize(&frame, t)?;
        Ok(rot.into())
    }
}

fn pixels(src: &IWICBitmapSource, size: u32) -> windows::core::Result<Vec<u8>> {
    let mut buf = vec![0u8; (size * size * 4) as usize];
    // SAFETY: buf is exactly size*size*4 bytes with stride size*4.
    unsafe { src.CopyPixels(std::ptr::null(), size * 4, &mut buf)? };
    Ok(buf)
}

/// A picture file as size x size premultiplied BGRA (square-cropped).
pub fn load_square(wic: &IWICImagingFactory, path: &Path, size: u32) -> windows::core::Result<Vec<u8>> {
    let src = open_oriented(wic, path)?;
    pixels(&fit_square(wic, &src, size, &GUID_WICPixelFormat32bppPBGRA)?, size)
}

fn save_png(wic: &IWICImagingFactory, src: &IWICBitmapSource, dst: &Path) -> windows::core::Result<()> {
    let tmp = dst.with_extension("png.tmp");
    // SAFETY: WIC PNG encode to a file stream; every object is used while alive.
    unsafe {
        let fitted = fit_square(wic, src, PICTURE_SIZE, &GUID_WICPixelFormat24bppBGR)?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(tmp.as_os_str()), GENERIC_WRITE.0)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        let mut bag = None;
        enc.CreateNewFrame(&mut frame, &mut bag)?;
        let frame = frame.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        frame.Initialize(bag.as_ref())?;
        frame.SetSize(PICTURE_SIZE, PICTURE_SIZE)?;
        let mut fmt = GUID_WICPixelFormat24bppBGR;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WriteSource(&fitted, std::ptr::null())?;
        frame.Commit()?;
        enc.Commit()?;
    }
    std::fs::rename(&tmp, dst).map_err(|e| windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string()))
}

/// Save a bitmap as a PNG at its own size (snapshots).
pub fn save_png_as_is(src: &IWICBitmapSource, dst: &Path) -> Result<(), String> {
    let wic = factory().map_err(|e| e.message())?;
    // SAFETY: WIC PNG encode to a file stream; every object is used while alive.
    unsafe {
        let (mut w, mut h) = (0, 0);
        src.GetSize(&mut w, &mut h).map_err(|e| e.message())?;
        let conv = wic.CreateFormatConverter().map_err(|e| e.message())?;
        conv.Initialize(src, &GUID_WICPixelFormat24bppBGR, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).map_err(|e| e.message())?;
        let stream = wic.CreateStream().map_err(|e| e.message())?;
        stream.InitializeFromFilename(&HSTRING::from(dst.as_os_str()), GENERIC_WRITE.0).map_err(|e| e.message())?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).map_err(|e| e.message())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache).map_err(|e| e.message())?;
        let mut frame = None;
        let mut bag = None;
        enc.CreateNewFrame(&mut frame, &mut bag).map_err(|e| e.message())?;
        let frame = frame.ok_or("no frame")?;
        frame.Initialize(bag.as_ref()).map_err(|e| e.message())?;
        frame.SetSize(w, h).map_err(|e| e.message())?;
        let mut fmt = GUID_WICPixelFormat24bppBGR;
        frame.SetPixelFormat(&mut fmt).map_err(|e| e.message())?;
        frame.WriteSource(&conv, std::ptr::null()).map_err(|e| e.message())?;
        frame.Commit().map_err(|e| e.message())?;
        enc.Commit().map_err(|e| e.message())?;
    }
    Ok(())
}

/// Save a picture file as a clip's picture (dst = "<clip>.png").
pub fn save_picture_from_file(src: &Path, dst: &Path) -> Result<(), String> {
    let wic = factory().map_err(|e| e.message())?;
    let s = open_oriented(&wic, src).map_err(|e| format!("can't read that picture ({})", e.message()))?;
    save_png(&wic, &s, dst).map_err(|e| e.message())
}

/// Save top-down straight-alpha BGRA pixels (a pasted image) as a clip's picture.
pub fn save_picture_from_pixels(w: u32, h: u32, bgra: &[u8], dst: &Path) -> Result<(), String> {
    let wic = factory().map_err(|e| e.message())?;
    // SAFETY: bgra is w*h*4 bytes with stride w*4.
    let bmp = unsafe { wic.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat32bppBGRA, w * 4, bgra) }.map_err(|e| e.message())?;
    save_png(&wic, &bmp.into(), dst).map_err(|e| e.message())
}

/// A tile picture to decode: (cache key, file, size in px).
pub struct Request {
    pub key: u64,
    pub path: PathBuf,
    pub px: u32,
}

pub struct Decoded {
    pub key: u64,
    pub px: u32,
    pub pixels: Option<Vec<u8>>,
}

/// Start the picture decoder thread. `wake` is called after each result is sent.
pub fn start_loader(results: Sender<Decoded>, wake: Box<dyn Fn() + Send>) -> Sender<Request> {
    let (tx, rx): (Sender<Request>, Receiver<Request>) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("pictures".into())
        .spawn(move || {
            let _com = crate::audio::wasapi::ComScope::mta();
            let Ok(wic) = factory() else { return };
            while let Ok(first) = rx.recv() {
                // newest requests first: what's on screen now matters most
                let mut batch = vec![first];
                while let Ok(r) = rx.try_recv() {
                    batch.push(r);
                }
                for r in batch.into_iter().rev() {
                    let pixels = load_square(&wic, &r.path, r.px).ok();
                    if results.send(Decoded { key: r.key, px: r.px, pixels }).is_err() {
                        return;
                    }
                    wake();
                }
            }
        })
        .expect("spawn picture loader");
    tx
}
