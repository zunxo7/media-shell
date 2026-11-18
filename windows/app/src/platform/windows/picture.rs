//! Pictures through WIC: each step (decode, crop, resize) a new bitmap source over the last, read out at the end.
use crate::platform::{Decoder, Error, Resample, Result};
use windows::core::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

thread_local! {
    /// This thread's WIC factory, made once (COM joined once per thread; the factory is free-threaded), not on every
    /// picture.
    static WIC: Option<IWICImagingFactory> = unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()
    };
}

fn wic() -> Result<IWICImagingFactory> {
    WIC.with(|f| f.clone()).ok_or_else(|| Error::new("WIC is unavailable"))
}

/// A picture on its way to pixels.
pub struct Picture(IWICBitmapSource);

impl Decoder for Picture {
    fn decode(data: &[u8]) -> Result<Picture> {
        let wic = wic()?;
        unsafe {
            let stream = wic.CreateStream()?;
            stream.InitializeFromMemory(data)?;
            let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
            Ok(Picture(decoder.GetFrame(0)?.cast()?))
        }
    }

    fn from_pixels(w: u32, h: u32, pixels: &[u8], alpha: bool) -> Option<Picture> {
        let wic = wic().ok()?;
        let (format, size) = if alpha { (GUID_WICPixelFormat32bppRGBA, 4) } else { (GUID_WICPixelFormat24bppRGB, 3) };
        unsafe { wic.CreateBitmapFromMemory(w, h, &format, w.checked_mul(size)?, pixels) }.ok()?.cast().ok().map(Picture)
    }

    fn size(&self) -> Result<(u32, u32)> {
        let (mut w, mut h) = (0, 0);
        unsafe { self.0.GetSize(&mut w, &mut h)? };
        Ok((w, h))
    }

    fn smaller(&self, w: u32, h: u32) -> Option<Picture> {
        unsafe {
            let wic = wic().ok()?;
            let transform: IWICBitmapSourceTransform = self.0.cast().ok()?;
            let (mut sw, mut sh) = (0u32, 0u32);
            self.0.GetSize(&mut sw, &mut sh).ok()?;
            if sw == 0 || sh == 0 || w == 0 || h == 0 {
                return None;
            }
            // the size it must keep: cropped to fill w×h, the crop at w×h
            let scale = (w as f64 / sw as f64).max(h as f64 / sh as f64);
            let (need_w, need_h) = ((sw as f64 * scale).ceil() as u32, (sh as f64 * scale).ceil() as u32);
            let format = self.0.GetPixelFormat().ok()?;
            let bytes = if format == GUID_WICPixelFormat24bppBGR {
                3
            } else if format == GUID_WICPixelFormat8bppGray {
                1
            } else {
                return None;
            };
            for d in [8u32, 4, 2] {
                let (mut tw, mut th) = (sw.div_ceil(d), sh.div_ceil(d));
                if tw < need_w || th < need_h {
                    continue;
                }
                transform.GetClosestSize(&mut tw, &mut th).ok()?;
                if tw < need_w || th < need_h || tw >= sw || th >= sh {
                    continue;
                }
                let mut want = format;
                transform.GetClosestPixelFormat(&mut want).ok()?;
                if want != format {
                    return None;
                }
                let stride = (tw * bytes).div_ceil(4) * 4;
                let mut buffer = vec![0u8; (stride * th) as usize];
                transform.CopyPixels(std::ptr::null(), tw, th, &want, WICBitmapTransformRotate0, stride, &mut buffer).ok()?;
                let bitmap = wic.CreateBitmapFromMemory(tw, th, &want, stride, &buffer).ok()?;
                return bitmap.cast().ok().map(Picture);
            }
            None
        }
    }

    fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Result<Picture> {
        unsafe {
            let clipper = wic()?.CreateBitmapClipper()?;
            clipper.Initialize(&self.0, &WICRect { X: x as i32, Y: y as i32, Width: w as i32, Height: h as i32 })?;
            Ok(Picture(clipper.cast()?))
        }
    }

    fn resize(&self, w: u32, h: u32, how: Resample) -> Result<Picture> {
        let mode = match how {
            Resample::Fant => WICBitmapInterpolationModeFant,
            Resample::Cubic => WICBitmapInterpolationModeHighQualityCubic,
        };
        unsafe {
            let scaler = wic()?.CreateBitmapScaler()?;
            scaler.Initialize(&self.0, w, h, mode)?;
            Ok(Picture(scaler.cast()?))
        }
    }

    fn read(&self, stride: u32, out: &mut [u8]) -> Result<()> {
        unsafe {
            let conv = wic()?.CreateFormatConverter()?;
            conv.Initialize(&self.0, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
            conv.CopyPixels(std::ptr::null(), stride, out)?;
            Ok(())
        }
    }
}
