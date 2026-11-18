//! Drawing into a surface with Core Graphics, in the surface's scaled units with y running down, as Direct2D draws
//! for Windows: shapes, gradients mixed in sRGB, Lucide's paths, text, decoded pixels.
use super::compositor::Graphics;
use crate::gfx::TextStyle;
use crate::platform::{self as seam, Paint, PathSink, Rect, Result};
use objc2_core_foundation::{CFData, CFRetained, CGAffineTransform, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapInfo, CGColor, CGColorRenderingIntent, CGColorSpace, CGContext, CGDataProvider, CGGradient, CGGradientDrawingOptions, CGImage,
    CGLineCap, CGLineJoin, CGMutablePath, CGPath,
};

pub(super) fn colour(argb: u32) -> CFRetained<CGColor> {
    let c = |shift: u32| (argb >> shift & 255) as CGFloat / 255.;
    CGColor::new_srgb(c(16), c(8), c(0), c(24))
}

pub(super) fn rect(r: Rect) -> CGRect {
    CGRect::new(CGPoint::new(r.x as f64, r.y as f64), CGSize::new(r.w as f64, r.h as f64))
}

pub(super) fn srgb() -> CFRetained<CGColorSpace> {
    CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB })).expect("sRGB")
}

/// A rounded box's outline, its radius held to half its shorter side (as Direct2D holds it).
fn rounded(r: Rect, radius: f32) -> CFRetained<CGPath> {
    let radius = (radius.max(0.).min(r.w / 2.).min(r.h / 2.)) as f64;
    unsafe { CGPath::with_rounded_rect(rect(r), radius, radius, std::ptr::null()) }
}

/// Drawing into a surface between its clearing and its showing.
pub struct Canvas<'a> {
    pub(super) cg: &'a CGContext,
    pub(super) g: &'a Graphics,
    /// The surface's scale (pixels per unit).
    pub(super) scale: f32,
}

impl Canvas<'_> {
    /// `path` filled (even-odd or not) with `paint`.
    fn fill(&self, path: &CGPath, even_odd: bool, paint: &Paint) -> Result<()> {
        let cg = Some(self.cg);
        match *paint {
            Paint::Solid(c) => {
                CGContext::set_fill_color_with_color(cg, Some(&colour(c)));
                CGContext::add_path(cg, Some(path));
                if even_odd { CGContext::eo_fill_path(cg) } else { CGContext::fill_path(cg) }
            }
            Paint::Linear { from, to, stops } => {
                CGContext::save_g_state(cg);
                CGContext::add_path(cg, Some(path));
                if even_odd { CGContext::eo_clip(cg) } else { CGContext::clip(cg) }
                let options = CGGradientDrawingOptions::DrawsBeforeStartLocation | CGGradientDrawingOptions::DrawsAfterEndLocation;
                CGContext::draw_linear_gradient(cg, gradient(stops).as_deref(), point(from), point(to), options);
                CGContext::restore_g_state(cg);
            }
            Paint::Radial { centre, radius, stops } => {
                CGContext::save_g_state(cg);
                CGContext::add_path(cg, Some(path));
                if even_odd { CGContext::eo_clip(cg) } else { CGContext::clip(cg) }
                // (an ellipse: a circle of the across radius, squashed down to the other)
                let (rx, ry) = (radius.0.max(1e-3) as f64, radius.1.max(1e-3) as f64);
                CGContext::translate_ctm(cg, centre.0 as f64, centre.1 as f64);
                CGContext::scale_ctm(cg, 1., ry / rx);
                let options = CGGradientDrawingOptions::DrawsBeforeStartLocation | CGGradientDrawingOptions::DrawsAfterEndLocation;
                CGContext::draw_radial_gradient(cg, gradient(stops).as_deref(), CGPoint::new(0., 0.), 0., CGPoint::new(0., 0.), rx, options);
                CGContext::restore_g_state(cg);
            }
        }
        Ok(())
    }

    fn stroke(&self, path: &CGPath, argb: u32, width: f32, cap: CGLineCap, join: CGLineJoin) {
        let cg = Some(self.cg);
        CGContext::set_stroke_color_with_color(cg, Some(&colour(argb)));
        CGContext::set_line_width(cg, width as f64);
        CGContext::set_line_cap(cg, cap);
        CGContext::set_line_join(cg, join);
        CGContext::add_path(cg, Some(path));
        CGContext::stroke_path(cg);
    }
}

fn point(p: (f32, f32)) -> CGPoint {
    CGPoint::new(p.0 as f64, p.1 as f64)
}

/// The stops as a gradient in sRGB, clamped at its ends.
fn gradient(stops: &[(f32, u32)]) -> Option<CFRetained<CGGradient>> {
    let mut components = Vec::with_capacity(stops.len() * 4);
    let mut locations = Vec::with_capacity(stops.len());
    for &(at, argb) in stops {
        let c = |shift: u32| (argb >> shift & 255) as CGFloat / 255.;
        components.extend([c(16), c(8), c(0), c(24)]);
        locations.push(at as CGFloat);
    }
    unsafe { CGGradient::with_color_components(Some(&srgb()), components.as_ptr(), locations.as_ptr(), stops.len()) }
}

impl seam::Draw for Canvas<'_> {
    fn fill_rounded(&mut self, r: Rect, radius: f32, paint: &Paint) -> Result<()> {
        self.fill(&rounded(r, radius), false, paint)
    }

    fn stroke_rounded(&mut self, r: Rect, radius: f32, paint: &Paint, width: f32) -> Result<()> {
        // (the line's outline filled, so a gradient strokes it as Direct2D's brush does)
        let path = rounded(r, radius);
        let outline = unsafe { CGPath::new_copy_by_stroking_path(Some(&path), std::ptr::null(), width as f64, CGLineCap::Butt, CGLineJoin::Miter, 10.) };
        match outline {
            Some(outline) => self.fill(&outline, false, paint),
            None => Ok(()),
        }
    }

    fn push_clip(&mut self, r: Rect, smooth: bool) {
        let cg = Some(self.cg);
        CGContext::save_g_state(cg);
        let mut r = rect(r);
        if !smooth {
            // (on whole pixels: the box's edges rounded in the device's own space)
            let k = self.scale as f64;
            let (x0, y0) = ((r.origin.x * k).round() / k, (r.origin.y * k).round() / k);
            let (x1, y1) = (((r.origin.x + r.size.width) * k).round() / k, ((r.origin.y + r.size.height) * k).round() / k);
            r = CGRect::new(CGPoint::new(x0, y0), CGSize::new(x1 - x0, y1 - y0));
        }
        CGContext::clip_to_rect(cg, r);
    }

    fn pop_clip(&mut self) {
        CGContext::restore_g_state(Some(self.cg));
    }

    fn fill_path(&mut self, d: &str, even_odd: bool, k: f32, dx: f32, dy: f32, paint: &Paint) -> Result<()> {
        let path = trace(d, Some(CGAffineTransform { a: k as f64, b: 0., c: 0., d: k as f64, tx: dx as f64, ty: dy as f64 }));
        self.fill(&path, even_odd, paint)
    }

    fn icon(&mut self, name: &'static str, x: f32, y: f32, size: f32, argb: u32) -> Result<()> {
        let cg = Some(self.cg);
        CGContext::save_g_state(cg);
        // (the icon's own 24-unit box, then the surface's scale)
        CGContext::translate_ctm(cg, x as f64, y as f64);
        CGContext::scale_ctm(cg, size as f64 / 24., size as f64 / 24.);
        for (path, filled, stroke) in self.g.icon_paths(name).iter() {
            if *filled {
                self.fill(path, false, &Paint::Solid(argb))?;
            }
            if *stroke > 0. {
                self.stroke(path, argb, *stroke, CGLineCap::Round, CGLineJoin::Round);
            }
        }
        CGContext::restore_g_state(cg);
        Ok(())
    }

    fn text(&mut self, text: &str, style: &TextStyle, w: f32, h: f32, argb: u32, at: &[(f32, f32)]) -> Result<()> {
        let layout = self.g.text().layout(text, style, w, h);
        let cg = Some(self.cg);
        CGContext::set_fill_color_with_color(cg, Some(&colour(argb)));
        for &(x, y) in at {
            self.g.text().draw(&layout, self.cg, x as f64, y as f64);
        }
        Ok(())
    }

    fn stroke_circle(&mut self, centre: (f32, f32), radius: f32, argb: u32, width: f32) -> Result<()> {
        let r = radius as f64;
        let path = unsafe {
            CGPath::with_ellipse_in_rect(CGRect::new(CGPoint::new(centre.0 as f64 - r, centre.1 as f64 - r), CGSize::new(2. * r, 2. * r)), std::ptr::null())
        };
        self.stroke(&path, argb, width, CGLineCap::Butt, CGLineJoin::Miter);
        Ok(())
    }

    fn stroke_arc(&mut self, from: (f32, f32), to: (f32, f32), radius: f32, large: bool, argb: u32, width: f32) -> Result<()> {
        let mut sink = Sink { path: CGMutablePath::new(), transform: None, at: from };
        sink.begin(from.0, from.1);
        sink.arc((radius, radius), 0., large, true, to);
        self.stroke(&sink.path, argb, width, CGLineCap::Round, CGLineJoin::Miter);
        Ok(())
    }

    fn pixels(&mut self, pixels: &[u8], pw: u32, ph: u32, w: f32, h: f32, radius: f32, at: (u32, u32)) -> Result<()> {
        let s = self.scale as f64;
        let Some(image) = bgra_image(pixels, pw, ph) else { return Ok(()) };
        let cg = Some(self.cg);
        CGContext::save_g_state(cg);
        // (moved along by whole pixels, then the picture 1:1 with them inside its rounded box)
        CGContext::translate_ctm(cg, at.0 as f64 / s, at.1 as f64 / s);
        CGContext::add_path(cg, Some(&rounded(Rect { x: 0., y: 0., w, h }, radius)));
        CGContext::clip(cg);
        // (drawn the right way up in a context whose y runs down)
        let (iw, ih) = (pw as f64 / s, ph as f64 / s);
        CGContext::translate_ctm(cg, 0., ih);
        CGContext::scale_ctm(cg, 1., -1.);
        CGContext::draw_image(cg, CGRect::new(CGPoint::new(0., 0.), CGSize::new(iw, ih)), Some(&image));
        CGContext::restore_g_state(cg);
        Ok(())
    }
}

/// Premultiplied BGRA rows (`pw` by `ph`) as an image.
pub(super) fn bgra_image(pixels: &[u8], pw: u32, ph: u32) -> Option<CFRetained<CGImage>> {
    let provider = CGDataProvider::with_cf_data(Some(&CFData::from_bytes(pixels)))?;
    let info = CGBitmapInfo(objc2_core_graphics::CGImageAlphaInfo::PremultipliedFirst.0 | objc2_core_graphics::CGImageByteOrderInfo::Order32Little.0);
    unsafe { CGImage::new(pw as usize, ph as usize, 8, 32, pw as usize * 4, Some(&srgb()), info, Some(&provider), std::ptr::null(), false, CGColorRenderingIntent::RenderingIntentDefault) }
}

/// An SVG path (`icons::trace`) as a Core Graphics path, each point moved by `transform`.
pub(super) fn trace(d: &str, transform: Option<CGAffineTransform>) -> CFRetained<CGMutablePath> {
    let mut sink = Sink { path: CGMutablePath::new(), transform, at: (0., 0.) };
    crate::icons::trace(d, &mut sink);
    sink.path
}

/// Traces into a path, remembering where the pen is (an arc needs its start).
struct Sink {
    path: CFRetained<CGMutablePath>,
    transform: Option<CGAffineTransform>,
    at: (f32, f32),
}

impl Sink {
    fn m(&self) -> *const CGAffineTransform {
        self.transform.as_ref().map_or(std::ptr::null(), |t| t as *const _)
    }
}

impl PathSink for Sink {
    fn begin(&mut self, x: f32, y: f32) {
        unsafe { CGMutablePath::move_to_point(Some(&self.path), self.m(), x as f64, y as f64) };
        self.at = (x, y);
    }
    fn line(&mut self, x: f32, y: f32) {
        unsafe { CGMutablePath::add_line_to_point(Some(&self.path), self.m(), x as f64, y as f64) };
        self.at = (x, y);
    }
    fn cubic(&mut self, c1: (f32, f32), c2: (f32, f32), to: (f32, f32)) {
        unsafe { CGMutablePath::add_curve_to_point(Some(&self.path), self.m(), c1.0 as f64, c1.1 as f64, c2.0 as f64, c2.1 as f64, to.0 as f64, to.1 as f64) };
        self.at = to;
    }
    fn quad(&mut self, c: (f32, f32), to: (f32, f32)) {
        unsafe { CGMutablePath::add_quad_curve_to_point(Some(&self.path), self.m(), c.0 as f64, c.1 as f64, to.0 as f64, to.1 as f64) };
        self.at = to;
    }
    /// SVG's endpoint arc turned into its centre and angles (SVG 1.1, F.6.5), then added as a turned, stretched
    /// unit circle's arc.
    fn arc(&mut self, radii: (f32, f32), rotation: f32, large: bool, clockwise: bool, to: (f32, f32)) {
        let (x1, y1) = (self.at.0 as f64, self.at.1 as f64);
        let (x2, y2) = (to.0 as f64, to.1 as f64);
        let (mut rx, mut ry) = (radii.0.abs() as f64, radii.1.abs() as f64);
        if rx == 0. || ry == 0. || (x1 == x2 && y1 == y2) {
            self.line(to.0, to.1);
            return;
        }
        let phi = (rotation as f64).to_radians();
        let (sin, cos) = phi.sin_cos();
        let (dx, dy) = ((x1 - x2) / 2., (y1 - y2) / 2.);
        let (x1p, y1p) = (cos * dx + sin * dy, -sin * dx + cos * dy);
        let grow = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
        if grow > 1. {
            rx *= grow.sqrt();
            ry *= grow.sqrt();
        }
        let num = (rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p).max(0.);
        let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
        let mut k = if den > 0. { (num / den).sqrt() } else { 0. };
        if large == clockwise {
            k = -k;
        }
        let (cxp, cyp) = (k * rx * y1p / ry, -k * ry * x1p / rx);
        let (cx, cy) = (cos * cxp - sin * cyp + (x1 + x2) / 2., sin * cxp + cos * cyp + (y1 + y2) / 2.);
        let angle = |ux: f64, uy: f64, vx: f64, vy: f64| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        let (ux, uy) = ((x1p - cxp) / rx, (y1p - cyp) / ry);
        let (vx, vy) = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
        let start = angle(1., 0., ux, uy);
        let mut delta = angle(ux, uy, vx, vy);
        if !clockwise && delta > 0. {
            delta -= std::f64::consts::TAU;
        } else if clockwise && delta < 0. {
            delta += std::f64::consts::TAU;
        }
        // (the unit circle stretched to the radii, turned, and put at the centre; then the path's own transform)
        let mut m = CGAffineTransform { a: cos * rx, b: sin * rx, c: -sin * ry, d: cos * ry, tx: cx, ty: cy };
        if let Some(t) = self.transform {
            m = CGAffineTransform {
                a: m.a * t.a + m.b * t.c,
                b: m.a * t.b + m.b * t.d,
                c: m.c * t.a + m.d * t.c,
                d: m.c * t.b + m.d * t.d,
                tx: m.tx * t.a + m.ty * t.c + t.tx,
                ty: m.tx * t.b + m.ty * t.d + t.ty,
            };
        }
        unsafe { CGMutablePath::add_relative_arc(Some(&self.path), &m, 0., 0., 1., start, delta) };
        self.at = to;
    }
    fn end(&mut self, closed: bool) {
        if closed {
            CGMutablePath::close_subpath(Some(&self.path));
        }
    }
}
