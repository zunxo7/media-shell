//! Pictures through ImageIO and Core Graphics: each step (decode, crop, resize) a new image, read out at the end.
use super::draw::srgb;
use crate::platform::{Decoder, Error, Resample, Result};
use objc2_core_foundation::{CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapInfo, CGBlendMode, CGColorRenderingIntent, CGContext, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGInterpolationQuality,
};
use objc2_image_io::{kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceThumbnailMaxPixelSize, CGImageSource};

/// A picture on its way to pixels; the JPEG it came from kept for reading it smaller.
pub struct Picture {
    image: CFRetained<CGImage>,
    source: Option<CFRetained<CGImageSource>>,
}

fn size_of(image: &CGImage) -> (u32, u32) {
    (CGImage::width(Some(image)) as u32, CGImage::height(Some(image)) as u32)
}

/// Premultiplied BGRA, the order the app's pixels are in.
const BGRA: u32 = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;

/// `image` drawn into `w` by `h` premultiplied BGRA pixels (`stride` bytes a row) at `out`, or a new buffer.
fn redraw(image: &CGImage, w: u32, h: u32, quality: CGInterpolationQuality, out: Option<(&mut [u8], u32)>) -> Result<Option<CFRetained<CGImage>>> {
    let (data, stride) = match out {
        Some((buffer, stride)) => {
            if buffer.len() < stride as usize * h as usize {
                return Err(Error::new("the picture's buffer is too small"));
            }
            (buffer.as_mut_ptr().cast(), stride as usize)
        }
        None => (std::ptr::null_mut(), 0),
    };
    let cg = unsafe { CGBitmapContextCreate(data, w as usize, h as usize, 8, stride, Some(&srgb()), BGRA) }
        .ok_or_else(|| Error::new(format!("no {w}x{h} picture")))?;
    let c = Some(&*cg);
    CGContext::set_blend_mode(c, CGBlendMode::Copy);
    CGContext::set_interpolation_quality(c, quality);
    CGContext::draw_image(c, CGRect::new(CGPoint::new(0., 0.), CGSize::new(w as f64, h as f64)), Some(image));
    Ok(if data.is_null() { objc2_core_graphics::CGBitmapContextCreateImage(c) } else { None })
}

impl Decoder for Picture {
    fn decode(data: &[u8]) -> Result<Picture> {
        let source = unsafe { CGImageSource::with_data(&CFData::from_bytes(data), None) }.ok_or_else(|| Error::new("not a picture"))?;
        let image = unsafe { source.image_at_index(0, None) }.ok_or_else(|| Error::new("the picture couldn't be read"))?;
        let jpeg = unsafe { source.r#type() }.is_some_and(|t| t.to_string() == "public.jpeg");
        Ok(Picture { image, source: jpeg.then_some(source) })
    }

    fn from_pixels(w: u32, h: u32, pixels: &[u8], alpha: bool) -> Option<Picture> {
        let size = if alpha { 4 } else { 3 };
        if pixels.len() < (w as usize * h as usize).checked_mul(size)? {
            return None;
        }
        let provider = CGDataProvider::with_cf_data(Some(&CFData::from_bytes(pixels)))?;
        let info = CGBitmapInfo(if alpha { CGImageAlphaInfo::Last.0 } else { CGImageAlphaInfo::None.0 });
        let image = unsafe {
            CGImage::new(w as usize, h as usize, 8, size * 8, w as usize * size, Some(&srgb()), info, Some(&provider), std::ptr::null(), false, CGColorRenderingIntent::RenderingIntentDefault)
        }?;
        Some(Picture { image, source: None })
    }

    fn size(&self) -> Result<(u32, u32)> {
        Ok(size_of(&self.image))
    }

    fn smaller(&self, w: u32, h: u32) -> Option<Picture> {
        let source = self.source.as_ref()?;
        let (sw, sh) = size_of(&self.image);
        if sw == 0 || sh == 0 || w == 0 || h == 0 {
            return None;
        }
        // the size it must keep: cropped to fill w×h, the crop at w×h
        let scale = (w as f64 / sw as f64).max(h as f64 / sh as f64);
        let (need_w, need_h) = ((sw as f64 * scale).ceil() as u32, (sh as f64 * scale).ceil() as u32);
        for d in [8u32, 4, 2] {
            let (tw, th) = (sw.div_ceil(d), sh.div_ceil(d));
            if tw < need_w || th < need_h {
                continue;
            }
            // (ImageIO reads a JPEG this small straight from its blocks, as WIC's transform does)
            let keys: [&CFString; 2] = unsafe { [kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceThumbnailMaxPixelSize] };
            let longest = CFNumber::new_i32(tw.max(th) as i32);
            let values: [&CFType; 2] = [CFBoolean::new(true).as_ref(), longest.as_ref()];
            let options = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
            let image = unsafe { source.thumbnail_at_index(0, Some(options.as_opaque())) }?;
            let (iw, ih) = size_of(&image);
            if iw < need_w || ih < need_h || iw >= sw || ih >= sh {
                continue;
            }
            return Some(Picture { image, source: None });
        }
        None
    }

    fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Result<Picture> {
        let r = CGRect::new(CGPoint::new(x as f64, y as f64), CGSize::new(w as f64, h as f64));
        let image = CGImage::with_image_in_rect(Some(&self.image), r).ok_or_else(|| Error::new("the crop is outside the picture"))?;
        Ok(Picture { image, source: None })
    }

    fn resize(&self, w: u32, h: u32, how: Resample) -> Result<Picture> {
        let quality = match how {
            Resample::Fant => CGInterpolationQuality::Medium,
            Resample::Cubic => CGInterpolationQuality::High,
        };
        let image = redraw(&self.image, w, h, quality, None)?.ok_or_else(|| Error::new("the picture couldn't be resized"))?;
        Ok(Picture { image, source: None })
    }

    fn read(&self, stride: u32, out: &mut [u8]) -> Result<()> {
        let (w, h) = size_of(&self.image);
        redraw(&self.image, w, h, CGInterpolationQuality::None, Some((out, stride)))?;
        Ok(())
    }
}
