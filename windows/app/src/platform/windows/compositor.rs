//! The compositor's visual tree, brushes and keyframe animations; Direct2D drawing into its surfaces, sharing one
//! device with it; DirectWrite text on the bundled fonts.
use super::window::Window;
use crate::gfx::{Font, TextStyle};
use crate::platform::{self as seam, Animation, Clip, Easing, Fit, Pair, Paint, PathSink, Rect, Result, Scalar};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use windows::core::*;
use windows::Foundation::{IPropertyValue, PropertyValue, Size, TimeSpan};
use windows::Graphics::DirectX::{DirectXAlphaMode, DirectXPixelFormat};
use windows::Graphics::Effects::*;
use windows::UI::Composition as wc;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::WinRT::Composition::*;
use windows::Win32::System::WinRT::Graphics::Direct2D::*;
use windows_numerics::{Matrix3x2, Matrix4x4, Vector2, Vector3};

/// CLSID_D2D1GaussianBlur
const GAUSSIAN_BLUR: GUID = GUID::from_u128(0x1feb6d69_2fe6_4ac9_8c58_1d7f93e7a6a5);
/// CLSID_D2D1ColorMatrix
const COLOR_MATRIX: GUID = GUID::from_u128(0x921f03d6_641c_47df_852d_b4bb6153ae11);

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}
fn ms(n: i64) -> TimeSpan {
    TimeSpan { Duration: n * 10_000 }
}
fn d2color(argb: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { a: (argb >> 24 & 255) as f32 / 255., r: (argb >> 16 & 255) as f32 / 255., g: (argb >> 8 & 255) as f32 / 255., b: (argb & 255) as f32 / 255. }
}
fn color(argb: u32) -> windows::UI::Color {
    windows::UI::Color { A: (argb >> 24) as u8, R: (argb >> 16) as u8, G: (argb >> 8) as u8, B: argb as u8 }
}
fn rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

/// A visual: a container, or a sprite that also shows a brush.
#[derive(Clone)]
pub struct Visual {
    v: wc::ContainerVisual,
    sprite: Option<wc::SpriteVisual>,
}

/// Anything a sprite can show.
#[derive(Clone)]
pub struct Brush(wc::CompositionBrush);

/// A colour that can glide to another (`animate_colour`).
#[derive(Clone)]
pub struct ColorBrush(wc::CompositionColorBrush);

/// A drawing surface the compositor shows through a brush.
#[derive(Clone)]
pub struct Surface(wc::CompositionDrawingSurface);

impl seam::Layer for Visual {
    fn set_offset(&self, x: f32, y: f32) -> Result<()> {
        Ok(self.v.SetOffset(Vector3 { X: x, Y: y, Z: 0. })?)
    }
    fn set_size(&self, w: f32, h: f32) -> Result<()> {
        Ok(self.v.SetSize(v2(w, h))?)
    }
    fn set_centre(&self, x: f32, y: f32) -> Result<()> {
        Ok(self.v.SetCenterPoint(Vector3 { X: x, Y: y, Z: 0. })?)
    }
    fn set_rotation(&self, degrees: f32) -> Result<()> {
        Ok(self.v.SetRotationAngleInDegrees(degrees)?)
    }
    fn set_opacity(&self, opacity: f32) -> Result<()> {
        Ok(self.v.SetOpacity(opacity)?)
    }
    fn set_visible(&self, visible: bool) -> Result<()> {
        Ok(self.v.SetIsVisible(visible)?)
    }
    fn set_transform(&self, dx: f32, dy: f32, k: f32) -> Result<()> {
        Ok(self.v.SetTransformMatrix(Matrix4x4 {
            M11: k, M12: 0., M13: 0., M14: 0.,
            M21: 0., M22: k, M23: 0., M24: 0.,
            M31: 0., M32: 0., M33: 1., M34: 0.,
            M41: dx, M42: dy, M43: 0., M44: 1.,
        })?)
    }
    fn set_brush(&self, brush: Option<&Brush>) -> Result<()> {
        if let Some(sprite) = &self.sprite {
            match brush {
                Some(b) => sprite.SetBrush(&b.0)?,
                None => sprite.SetBrush(None)?,
            }
        }
        Ok(())
    }
    fn stop(&self, prop: Scalar) -> Result<()> {
        Ok(self.v.StopAnimation(name(prop))?)
    }
    fn seek(&self, prop: Scalar, progress: f32) -> Result<()> {
        if let Ok(c) = self.v.TryGetAnimationController(name(prop)) {
            c.SetProgress(progress)?;
        }
        Ok(())
    }
    fn insert_top(&self, child: &Visual) -> Result<()> {
        Ok(self.v.Children()?.InsertAtTop(&child.v)?)
    }
    fn insert_bottom(&self, child: &Visual) -> Result<()> {
        Ok(self.v.Children()?.InsertAtBottom(&child.v)?)
    }
    fn insert_above(&self, child: &Visual, sibling: &Visual) -> Result<()> {
        Ok(self.v.Children()?.InsertAbove(&child.v, &sibling.v)?)
    }
    fn remove(&self, child: &Visual) -> Result<()> {
        Ok(self.v.Children()?.Remove(&child.v)?)
    }
    fn remove_all(&self) -> Result<()> {
        Ok(self.v.Children()?.RemoveAll()?)
    }
    fn has_children(&self) -> Result<bool> {
        Ok(self.v.Children()?.Count()? > 0)
    }
}

/// The compositor property a scalar is.
fn name(prop: Scalar) -> &'static HSTRING {
    match prop {
        Scalar::OffsetX => h!("Offset.X"),
        Scalar::OffsetY => h!("Offset.Y"),
        Scalar::Opacity => h!("Opacity"),
        Scalar::Rotation => h!("RotationAngleInDegrees"),
        Scalar::ClipLeft => h!("LeftInset"),
        Scalar::ClipRight => h!("RightInset"),
    }
}

/// Measured with DirectWrite's own layout and drawn with it: the text formats made once each.
struct Text {
    dw: IDWriteFactory5,
    fonts: IDWriteFontCollection1,
    formats: RefCell<HashMap<(Font, u32), IDWriteTextFormat>>,
}

impl Text {
    fn format(&self, font: Font, size: f32) -> Result<IDWriteTextFormat> {
        let key = (font, (size * 16.) as u32);
        if let Some(f) = self.formats.borrow().get(&key) {
            return Ok(f.clone());
        }
        let weight = match font {
            Font::Sans => 400,
            Font::Medium => 500,
            Font::Bold => 600,
        };
        let f = unsafe { self.dw.CreateTextFormat(w!("Geist"), &self.fonts, DWRITE_FONT_WEIGHT(weight), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))? };
        self.formats.borrow_mut().insert(key, f.clone());
        Ok(f)
    }

    fn layout(&self, text: &str, t: &TextStyle, w: f32, h: f32) -> Result<IDWriteTextLayout> {
        unsafe {
            let wide: Vec<u16> = text.encode_utf16().collect();
            let f = self.format(t.font, t.size)?;
            let l = self.dw.CreateTextLayout(&wide, &f, w.max(1.), h.max(1.))?;
            if t.line > 0. {
                l.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, t.line, t.line * 0.8)?;
            }
            if t.spacing != 0. {
                let l1: IDWriteTextLayout1 = l.cast()?;
                l1.SetCharacterSpacing(0., t.spacing, 0., DWRITE_TEXT_RANGE { startPosition: 0, length: wide.len() as u32 })?;
            }
            if t.centre {
                l.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            }
            if t.lines == 1 {
                l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            }
            if t.lines > 0 {
                let sign = self.dw.CreateEllipsisTrimmingSign(&f)?;
                l.SetTrimming(&DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 }, &sign)?;
            }
            Ok(l)
        }
    }
}

/// One path of an icon: its shape, whether it's filled, and its stroke width (0 for none).
type IconPath = (ID2D1PathGeometry1, bool, f32);

/// The compositor on the window, one Direct2D device shared with it, and the app's fonts.
pub struct Graphics {
    c: wc::Compositor,
    _target: wc::Desktop::DesktopWindowTarget,
    root: Visual,
    device: wc::CompositionGraphicsDevice,
    factory: ID2D1Factory1,
    text: Text,
    /// The few stroke styles drawing uses, made once: round (icons), capped (arcs).
    strokes: RefCell<HashMap<u8, ID2D1StrokeStyle1>>,
    icons: RefCell<HashMap<&'static str, Vec<IconPath>>>,
    curves: RefCell<HashMap<[u32; 4], wc::CompositionEasingFunction>>,
    linear: RefCell<Option<wc::CompositionEasingFunction>>,
    step: RefCell<Option<wc::CompositionEasingFunction>>,
    /// The glass's effect, made once for each blur.
    blurs: RefCell<HashMap<(u32, u32), wc::CompositionEffectFactory>>,
}

impl Graphics {
    unsafe fn devices() -> Result<(ID2D1Factory1, ID2D1Device)> {
        let mut d3d = None;
        let create = |driver, device: &mut Option<ID3D11Device>| {
            D3D11CreateDevice(None, driver, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(device), None, None)
        };
        if create(D3D_DRIVER_TYPE_HARDWARE, &mut d3d).is_err() {
            create(D3D_DRIVER_TYPE_WARP, &mut d3d)?;
        }
        let d3d: ID3D11Device = d3d.unwrap();
        let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dev = factory.CreateDevice(&d3d.cast::<IDXGIDevice>()?)?;
        Ok((factory, dev))
    }

    fn easing(&self, e: Easing) -> Result<wc::CompositionEasingFunction> {
        Ok(match e {
            Easing::Linear => {
                let mut l = self.linear.borrow_mut();
                if l.is_none() {
                    *l = Some(self.c.CreateLinearEasingFunction()?.cast()?);
                }
                l.clone().unwrap()
            }
            Easing::Step => {
                let mut s = self.step.borrow_mut();
                if s.is_none() {
                    *s = Some(self.c.CreateStepEasingFunction()?.cast()?);
                }
                s.clone().unwrap()
            }
            Easing::Bezier(b) => {
                let key = b.map(f32::to_bits);
                if let Some(c) = self.curves.borrow().get(&key) {
                    return Ok(c.clone());
                }
                let c: wc::CompositionEasingFunction = self.c.CreateCubicBezierEasingFunction(v2(b[0], b[1]), v2(b[2], b[3]))?.cast()?;
                self.curves.borrow_mut().insert(key, c.clone());
                c
            }
        })
    }

    /// The keyframes put in (a keyframe at 0 has no curve into it), then the timing.
    fn keyframes<V: Copy, A: Interface>(&self, made: A, a: &Animation<V>, insert: impl Fn(&A, f32, V, Option<&wc::CompositionEasingFunction>) -> windows::core::Result<()>) -> Result<A> {
        for &(at, v, e) in &a.frames {
            if at == 0. {
                insert(&made, at, v, None)?;
            } else {
                insert(&made, at, v, Some(&self.easing(e)?))?;
            }
        }
        let k: wc::KeyFrameAnimation = made.cast()?;
        k.SetDuration(ms(a.duration_ms))?;
        if a.delay_ms != 0 {
            k.SetDelayTime(ms(a.delay_ms))?;
            k.SetDelayBehavior(wc::AnimationDelayBehavior::SetInitialValueBeforeDelay)?;
        }
        if a.forever {
            k.SetIterationBehavior(wc::AnimationIterationBehavior::Forever)?;
        }
        Ok(made)
    }

    fn scalar(&self, a: &Animation<f32>) -> Result<wc::ScalarKeyFrameAnimation> {
        self.keyframes(self.c.CreateScalarKeyFrameAnimation()?, a, |x, at, v, e| match e {
            Some(e) => x.InsertKeyFrameWithEasingFunction(at, v, e),
            None => x.InsertKeyFrame(at, v),
        })
    }

    fn icon_paths(&self, name: &'static str) -> Result<()> {
        if self.icons.borrow().contains_key(name) {
            return Ok(());
        }
        let mut paths = Vec::new();
        for part in crate::icons::parts(name) {
            paths.push((self.path(part.d, false, part.filled)?, part.filled, part.stroke));
        }
        self.icons.borrow_mut().insert(name, paths);
        Ok(())
    }

    /// An SVG path as a Direct2D geometry: its figures filled or hollow, even-odd or winding.
    fn path(&self, d: &str, even_odd: bool, filled: bool) -> Result<ID2D1PathGeometry1> {
        unsafe {
            let g = self.factory.CreatePathGeometry()?;
            let sink = g.Open()?;
            sink.SetFillMode(if even_odd { D2D1_FILL_MODE_ALTERNATE } else { D2D1_FILL_MODE_WINDING });
            crate::icons::trace(d, &mut Sink { sink: &sink, begin: if filled { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW } });
            sink.Close()?;
            Ok(g)
        }
    }

    /// A stroke style, made once: 0 round all over (icons), 1 round caps (arcs).
    fn stroke(&self, kind: u8) -> Result<ID2D1StrokeStyle1> {
        if let Some(s) = self.strokes.borrow().get(&kind) {
            return Ok(s.clone());
        }
        let props = match kind {
            0 => D2D1_STROKE_STYLE_PROPERTIES1 { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, dashCap: D2D1_CAP_STYLE_ROUND, lineJoin: D2D1_LINE_JOIN_ROUND, ..Default::default() },
            _ => D2D1_STROKE_STYLE_PROPERTIES1 { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, ..Default::default() },
        };
        let s = unsafe { self.factory.CreateStrokeStyle(&props, None)? };
        self.strokes.borrow_mut().insert(kind, s.clone());
        Ok(s)
    }
}

impl seam::Compositor for Graphics {
    fn new(window: &Window, font_dir: &Path) -> Result<Graphics> {
        let c = wc::Compositor::new()?;
        let target = unsafe { c.cast::<ICompositorDesktopInterop>()?.CreateDesktopWindowTarget(window.hwnd(), false)? };
        let root = c.CreateContainerVisual()?;
        target.SetRoot(&root)?;
        unsafe {
            let (factory, dev) = Self::devices()?;
            let device = c.cast::<ICompositorInterop>()?.CreateGraphicsDevice(&dev)?;
            let dw: IDWriteFactory5 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            // The app's own fonts, straight from the files: nothing is installed.
            let builder = dw.CreateFontSetBuilder()?;
            for f in std::fs::read_dir(font_dir).map_err(|e| seam::Error::new(e.to_string()))? {
                let p = f.map_err(|e| seam::Error::new(e.to_string()))?.path();
                builder.AddFontFile(&dw.CreateFontFileReference(&HSTRING::from(p.as_os_str()), None)?)?;
            }
            let fonts = dw.CreateFontCollectionFromFontSet(&builder.CreateFontSet()?)?;
            Ok(Graphics {
                c,
                _target: target,
                root: Visual { v: root, sprite: None },
                device,
                factory,
                text: Text { dw, fonts, formats: Default::default() },
                strokes: Default::default(),
                icons: Default::default(),
                curves: Default::default(),
                linear: Default::default(),
                step: Default::default(),
                blurs: Default::default(),
            })
        }
    }

    fn root(&self) -> &Visual {
        &self.root
    }

    fn sprite(&self) -> Result<Visual> {
        let s = self.c.CreateSpriteVisual()?;
        Ok(Visual { v: s.cast()?, sprite: Some(s) })
    }

    fn container(&self) -> Result<Visual> {
        Ok(Visual { v: self.c.CreateContainerVisual()?, sprite: None })
    }

    fn clip(&self, visual: &Visual, clip: Clip) -> Result<()> {
        match clip {
            Clip::None => visual.v.SetClip(None)?,
            Clip::Inset => visual.v.SetClip(&self.c.CreateInsetClip()?)?,
            Clip::Rounded { w, h, radius } => {
                let geo = self.c.CreateRoundedRectangleGeometry()?;
                geo.SetSize(v2(w, h))?;
                geo.SetCornerRadius(v2(radius, radius))?;
                visual.v.SetClip(&self.c.CreateGeometricClipWithGeometry(&geo)?)?;
            }
        }
        Ok(())
    }

    fn color_brush(&self, argb: u32) -> Result<ColorBrush> {
        Ok(ColorBrush(self.c.CreateColorBrushWithColor(color(argb))?))
    }

    fn solid(&self, argb: u32) -> Result<Brush> {
        Ok(Brush(self.c.CreateColorBrushWithColor(color(argb))?.cast()?))
    }

    fn surface_brush(&self, surface: &Surface, fit: Fit) -> Result<Brush> {
        let b = self.c.CreateSurfaceBrushWithSurface(&surface.0)?;
        match fit {
            Fit::Stretch => b.SetStretch(wc::CompositionStretch::Fill)?,
            Fit::At { offset, scale } => {
                b.SetStretch(wc::CompositionStretch::None)?;
                b.SetHorizontalAlignmentRatio(0.)?;
                b.SetVerticalAlignmentRatio(0.)?;
                if scale != [1., 1.] {
                    b.SetScale(v2(scale[0], scale[1]))?;
                }
                b.SetOffset(v2(offset[0], offset[1]))?;
            }
        }
        Ok(Brush(b.cast()?))
    }

    fn masked(&self, colour: &ColorBrush, mask: &Brush) -> Result<Brush> {
        let m = self.c.CreateMaskBrush()?;
        m.SetSource(&colour.0)?;
        m.SetMask(&mask.0)?;
        Ok(Brush(m.cast()?))
    }

    fn backdrop_blur(&self, sigma: f32, saturation: f32) -> Result<Brush> {
        let key = (sigma.to_bits(), saturation.to_bits());
        if !self.blurs.borrow().contains_key(&key) {
            let source: IGraphicsEffectSource = wc::CompositionEffectSourceParameter::Create(h!("Backdrop"))?.cast()?;
            let vivid: IGraphicsEffectSource = Effect::saturate(saturation, source).into();
            let effect: IGraphicsEffect = Effect::blur(sigma, vivid).into();
            self.blurs.borrow_mut().insert(key, self.c.CreateEffectFactory(&effect)?);
        }
        let brush = self.blurs.borrow()[&key].CreateBrush()?;
        brush.SetSourceParameter(h!("Backdrop"), &self.c.CreateBackdropBrush()?)?;
        Ok(Brush(brush.cast()?))
    }

    fn video_brush(&self, video: super::video::Surface) -> Result<Brush> {
        let swap = video.into_swap_chain();
        let surface = unsafe { self.c.cast::<ICompositorInterop>()?.CreateCompositionSurfaceForSwapChain(&swap)? };
        let b = self.c.CreateSurfaceBrushWithSurface(&surface)?;
        let _ = b.SetStretch(wc::CompositionStretch::Fill);
        Ok(Brush(b.cast()?))
    }

    fn animate(&self, visual: &Visual, prop: Scalar, a: &Animation<f32>) -> Result<()> {
        let anim = self.scalar(a)?;
        match prop {
            Scalar::ClipLeft | Scalar::ClipRight => {
                if let Ok(clip) = visual.v.Clip().and_then(|c| c.cast::<wc::InsetClip>()) {
                    clip.StartAnimation(name(prop), &anim)?;
                }
            }
            _ => visual.v.StartAnimation(name(prop), &anim)?,
        }
        Ok(())
    }

    fn animate_pair(&self, visual: &Visual, prop: Pair, a: &Animation<[f32; 2]>) -> Result<()> {
        // (an offset lies in the plane; a scale leaves depth as it is)
        let z = if prop == Pair::Scale { 1. } else { 0. };
        let anim = self.keyframes(self.c.CreateVector3KeyFrameAnimation()?, a, |x, at, [vx, vy], e| {
            let v = Vector3 { X: vx, Y: vy, Z: z };
            match e {
                Some(e) => x.InsertKeyFrameWithEasingFunction(at, v, e),
                None => x.InsertKeyFrame(at, v),
            }
        })?;
        let prop = if prop == Pair::Scale { h!("Scale") } else { h!("Offset") };
        Ok(visual.v.StartAnimation(prop, &anim)?)
    }

    fn animate_colour(&self, brush: &ColorBrush, a: &Animation<u32>) -> Result<()> {
        let anim = self.keyframes(self.c.CreateColorKeyFrameAnimation()?, a, |x, at, c, e| match e {
            Some(e) => x.InsertKeyFrameWithEasingFunction(at, color(c), e),
            None => x.InsertKeyFrame(at, color(c)),
        })?;
        Ok(brush.0.StartAnimation(h!("Color"), &anim)?)
    }

    fn animate_brush_offset(&self, brush: &Brush, a: &Animation<[f32; 2]>) -> Result<()> {
        let anim = self.keyframes(self.c.CreateVector2KeyFrameAnimation()?, a, |x, at, [vx, vy], e| match e {
            Some(e) => x.InsertKeyFrameWithEasingFunction(at, v2(vx, vy), e),
            None => x.InsertKeyFrame(at, v2(vx, vy)),
        })?;
        Ok(brush.0.StartAnimation(h!("Offset"), &anim)?)
    }

    fn play_brush(&self, brush: &Brush, playing: bool) -> Result<bool> {
        let Ok(c) = brush.0.TryGetAnimationController(h!("Offset")) else { return Ok(false) };
        if playing { c.Resume()? } else { c.Pause()? }
        Ok(true)
    }

    fn surface(&self, pw: u32, ph: u32) -> Result<Surface> {
        Ok(Surface(self.device.CreateDrawingSurface(Size { Width: pw as f32, Height: ph as f32 }, DirectXPixelFormat::B8G8R8A8UIntNormalized, DirectXAlphaMode::Premultiplied)?))
    }

    fn paint(&self, surface: &Surface, scale: f32, draw: impl FnOnce(&mut Canvas<'_>) -> Result<()>) -> Result<bool> {
        unsafe {
            let interop: ICompositionDrawingSurfaceInterop = surface.0.cast()?;
            let mut off = POINT::default();
            let dc: ID2D1DeviceContext = interop.BeginDraw(None, &mut off)?;
            dc.SetDpi(96.0, 96.0);
            dc.SetTransform(&Matrix3x2 { M11: scale, M12: 0., M21: 0., M22: scale, M31: off.x as f32, M32: off.y as f32 });
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc.Clear(Some(&D2D1_COLOR_F { r: 0., g: 0., b: 0., a: 0. }));
            let r = draw(&mut Canvas { dc: &dc, g: self, scale });
            interop.EndDraw()?;
            r.map(|_| true)
        }
    }

    fn paint_in(&self, surface: &Surface, x: u32, y: u32, w: u32, h: u32, scale: f32, draw: impl FnOnce(&mut Canvas<'_>) -> Result<()>) -> Result<bool> {
        unsafe {
            let interop: ICompositionDrawingSurfaceInterop = surface.0.cast()?;
            let mut off = POINT::default();
            // (the part and the pixel all round it: that border is what a skin shown a touch smaller or bigger (a press,
            // a hover's growth) samples past its edge, so it's cleared too; left as it was, a part used before by
            // something else showed through as thin lines along the edges)
            let (lx, ty) = (x.min(1), y.min(1));
            let area = RECT { left: (x - lx) as i32, top: (y - ty) as i32, right: (x + w + 1) as i32, bottom: (y + h + 1) as i32 };
            let dc: ID2D1DeviceContext = interop.BeginDraw(Some(&area), &mut off)?;
            dc.SetDpi(96.0, 96.0);
            dc.SetTransform(&Matrix3x2 { M11: 1., M12: 0., M21: 0., M22: 1., M31: off.x as f32, M32: off.y as f32 });
            dc.Clear(Some(&D2D1_COLOR_F { r: 0., g: 0., b: 0., a: 0. }));
            // (held to its part, in whole pixels, before the scale goes on: the drawing stays inside it)
            let (ox, oy) = (off.x as f32 + lx as f32, off.y as f32 + ty as f32);
            dc.SetTransform(&Matrix3x2 { M11: 1., M12: 0., M21: 0., M22: 1., M31: ox, M32: oy });
            dc.PushAxisAlignedClip(&rect(Rect { x: 0., y: 0., w: w as f32, h: h as f32 }), D2D1_ANTIALIAS_MODE_ALIASED);
            dc.SetTransform(&Matrix3x2 { M11: scale, M12: 0., M21: 0., M22: scale, M31: ox, M32: oy });
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let r = draw(&mut Canvas { dc: &dc, g: self, scale });
            dc.PopAxisAlignedClip();
            interop.EndDraw()?;
            r.map(|_| true)
        }
    }

    fn replace_device(&mut self) -> Result<()> {
        unsafe {
            let (factory, dev) = Self::devices()?;
            self.device.cast::<ICompositionGraphicsDeviceInterop>()?.SetRenderingDevice(&dev)?;
            self.factory = factory;
            self.icons.borrow_mut().clear();
            self.strokes.borrow_mut().clear();
            Ok(())
        }
    }

    fn text_size(&self, text: &str, style: &TextStyle, width: f32) -> Result<(f32, f32)> {
        unsafe {
            let l = self.text.layout(text, style, width, 1e6)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            l.GetMetrics(&mut m)?;
            Ok((m.widthIncludingTrailingWhitespace, m.height))
        }
    }

    fn caret_x(&self, text: &str, style: &TextStyle, at: usize) -> Result<f32> {
        unsafe {
            let l = self.text.layout(text, style, 1e5, 100.)?;
            let (mut x, mut y) = (0f32, 0f32);
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            l.HitTestTextPosition(at as u32, false, &mut x, &mut y, &mut m)?;
            Ok(x)
        }
    }

    fn hit_char(&self, text: &str, style: &TextStyle, x: f32) -> Result<usize> {
        unsafe {
            let l = self.text.layout(text, style, 1e5, 100.)?;
            let (mut trailing, mut inside) = (BOOL(0), BOOL(0));
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            l.HitTestPoint(x, 1., &mut trailing, &mut inside, &mut m)?;
            Ok(m.textPosition as usize + if trailing.as_bool() { m.length as usize } else { 0 })
        }
    }
}

/// Drawing into a surface between its BeginDraw and EndDraw.
pub struct Canvas<'a> {
    dc: &'a ID2D1DeviceContext,
    g: &'a Graphics,
    /// The surface's scale (pixels per unit).
    scale: f32,
}

impl Canvas<'_> {
    fn brush(&self, p: &Paint) -> Result<ID2D1Brush> {
        unsafe {
            let stops = |list: &[(f32, u32)]| -> Result<ID2D1GradientStopCollection> {
                let s: Vec<D2D1_GRADIENT_STOP> = list.iter().map(|(p, c)| D2D1_GRADIENT_STOP { position: *p, color: d2color(*c) }).collect();
                let target: &ID2D1RenderTarget = self.dc;
                Ok(target.CreateGradientStopCollection(&s, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?)
            };
            Ok(match *p {
                Paint::Solid(c) => self.dc.CreateSolidColorBrush(&d2color(c), None)?.cast()?,
                Paint::Linear { from, to, stops: list } => self
                    .dc
                    .CreateLinearGradientBrush(&D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: v2(from.0, from.1), endPoint: v2(to.0, to.1) }, None, &stops(list)?)?
                    .cast()?,
                Paint::Radial { centre, radius, stops: list } => self
                    .dc
                    .CreateRadialGradientBrush(
                        &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES { center: v2(centre.0, centre.1), gradientOriginOffset: v2(0., 0.), radiusX: radius.0, radiusY: radius.1 },
                        None,
                        &stops(list)?,
                    )?
                    .cast()?,
            })
        }
    }
}

impl seam::Draw for Canvas<'_> {
    fn fill_rounded(&mut self, r: Rect, radius: f32, paint: &Paint) -> Result<()> {
        let b = self.brush(paint)?;
        unsafe { self.dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius }, &b) };
        Ok(())
    }

    fn stroke_rounded(&mut self, r: Rect, radius: f32, paint: &Paint, width: f32) -> Result<()> {
        let b = self.brush(paint)?;
        unsafe { self.dc.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius }, &b, width, None) };
        Ok(())
    }

    fn push_clip(&mut self, r: Rect, smooth: bool) {
        unsafe { self.dc.PushAxisAlignedClip(&rect(r), if smooth { D2D1_ANTIALIAS_MODE_PER_PRIMITIVE } else { D2D1_ANTIALIAS_MODE_ALIASED }) };
    }

    fn pop_clip(&mut self) {
        unsafe { self.dc.PopAxisAlignedClip() };
    }

    fn fill_path(&mut self, d: &str, even_odd: bool, k: f32, dx: f32, dy: f32, paint: &Paint) -> Result<()> {
        let geo = self.g.path(d, even_odd, true)?;
        unsafe {
            let shaped = self.g.factory.CreateTransformedGeometry(&geo, &Matrix3x2 { M11: k, M12: 0., M21: 0., M22: k, M31: dx, M32: dy })?;
            let b = self.brush(paint)?;
            self.dc.FillGeometry(&shaped, &b, None);
        }
        Ok(())
    }

    fn icon(&mut self, name: &'static str, x: f32, y: f32, size: f32, argb: u32) -> Result<()> {
        self.g.icon_paths(name)?;
        let icons = self.g.icons.borrow();
        unsafe {
            let b = self.dc.CreateSolidColorBrush(&d2color(argb), None)?;
            let mut old = Matrix3x2::default();
            self.dc.GetTransform(&mut old);
            let k = size / 24.;
            let m = Matrix3x2 { M11: k, M12: 0., M21: 0., M22: k, M31: x, M32: y };
            // (the icon's own box first, then the surface's scale)
            self.dc.SetTransform(&mul(&m, &old));
            let style = self.g.stroke(0)?;
            for (g, filled, stroke) in &icons[name] {
                if *filled {
                    self.dc.FillGeometry(g, &b, None);
                }
                if *stroke > 0. {
                    self.dc.DrawGeometry(g, &b, *stroke, &style);
                }
            }
            self.dc.SetTransform(&old);
        }
        Ok(())
    }

    fn text(&mut self, text: &str, style: &TextStyle, w: f32, h: f32, argb: u32, at: &[(f32, f32)]) -> Result<()> {
        let l = self.g.text.layout(text, style, w, h)?;
        unsafe {
            let b = self.dc.CreateSolidColorBrush(&d2color(argb), None)?;
            for &(x, y) in at {
                self.dc.DrawTextLayout(v2(x, y), &l, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        }
        Ok(())
    }

    fn stroke_circle(&mut self, centre: (f32, f32), radius: f32, argb: u32, width: f32) -> Result<()> {
        unsafe {
            let b = self.dc.CreateSolidColorBrush(&d2color(argb), None)?;
            self.dc.DrawEllipse(&D2D1_ELLIPSE { point: v2(centre.0, centre.1), radiusX: radius, radiusY: radius }, &b, width, None);
        }
        Ok(())
    }

    fn stroke_arc(&mut self, from: (f32, f32), to: (f32, f32), radius: f32, large: bool, argb: u32, width: f32) -> Result<()> {
        unsafe {
            let b = self.dc.CreateSolidColorBrush(&d2color(argb), None)?;
            let round = self.g.stroke(1)?;
            let path = self.g.factory.CreatePathGeometry()?;
            let sink = path.Open()?;
            sink.BeginFigure(v2(from.0, from.1), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v2(to.0, to.1),
                size: D2D_SIZE_F { width: radius, height: radius },
                rotationAngle: 0.,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if large { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            self.dc.DrawGeometry(&path, &b, width, &round);
        }
        Ok(())
    }

    fn pixels(&mut self, pixels: &[u8], pw: u32, ph: u32, w: f32, h: f32, radius: f32, at: (u32, u32)) -> Result<()> {
        let s = self.scale;
        unsafe {
            let mut base = Matrix3x2::default();
            self.dc.GetTransform(&mut base);
            self.dc.SetTransform(&Matrix3x2 { M31: base.M31 + at.0 as f32, M32: base.M32 + at.1 as f32, ..base });
            let props = D2D1_BITMAP_PROPERTIES1 { pixelFormat: D2D1_PIXEL_FORMAT { format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96., dpiY: 96., ..Default::default() };
            let bmp = self.dc.CreateBitmap(D2D_SIZE_U { width: pw, height: ph }, Some(pixels.as_ptr() as *const _), pw * 4, &props)?;
            let brush = self.dc.CreateBitmapBrush(&bmp, None, None)?;
            brush.SetTransform(&Matrix3x2 { M11: 1. / s, M12: 0., M21: 0., M22: 1. / s, M31: 0., M32: 0. });
            let box_ = D2D1_ROUNDED_RECT { rect: rect(Rect { x: 0., y: 0., w, h }), radiusX: radius, radiusY: radius };
            self.dc.FillRoundedRectangle(&box_, &brush);
            self.dc.SetTransform(&base);
        }
        Ok(())
    }
}

fn mul(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21,
        M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21,
        M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31,
        M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}

/// An SVG path traced into a Direct2D geometry.
struct Sink<'a> {
    sink: &'a ID2D1GeometrySink,
    begin: D2D1_FIGURE_BEGIN,
}

impl PathSink for Sink<'_> {
    fn begin(&mut self, x: f32, y: f32) {
        unsafe { self.sink.BeginFigure(v2(x, y), self.begin) };
    }
    fn line(&mut self, x: f32, y: f32) {
        unsafe { self.sink.AddLine(v2(x, y)) };
    }
    fn cubic(&mut self, c1: (f32, f32), c2: (f32, f32), to: (f32, f32)) {
        unsafe { self.sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: v2(c1.0, c1.1), point2: v2(c2.0, c2.1), point3: v2(to.0, to.1) }) };
    }
    fn quad(&mut self, c: (f32, f32), to: (f32, f32)) {
        unsafe { self.sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: v2(c.0, c.1), point2: v2(to.0, to.1) }) };
    }
    fn arc(&mut self, radii: (f32, f32), rotation: f32, large: bool, clockwise: bool, to: (f32, f32)) {
        unsafe {
            self.sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v2(to.0, to.1),
                size: D2D_SIZE_F { width: radii.0, height: radii.1 },
                rotationAngle: rotation,
                sweepDirection: if clockwise { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
                arcSize: if large { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            })
        };
    }
    fn end(&mut self, closed: bool) {
        unsafe { self.sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN }) };
    }
}

enum Prop {
    F(f32),
    U(u32),
    B(bool),
    Floats(Vec<f32>),
}

/// A Direct2D effect in the shape the compositor asks for: Windows gives no ready-made ones outside Win2D.
#[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
struct Effect {
    name: RefCell<HSTRING>,
    id: GUID,
    props: Vec<Prop>,
    sources: Vec<IGraphicsEffectSource>,
}

impl Effect {
    /// Gaussian blur: StandardDeviation, Optimization (1 = balanced), BorderMode (1 = hard: no dark rim at the edge).
    fn blur(sigma: f32, source: IGraphicsEffectSource) -> Effect {
        Effect { name: RefCell::new(HSTRING::from("blur")), id: GAUSSIAN_BLUR, props: vec![Prop::F(sigma), Prop::U(1), Prop::U(1)], sources: vec![source] }
    }
    /// CSS `saturate(s)`, as a colour matrix (the D2D saturation effect stops at 1).
    fn saturate(s: f32, source: IGraphicsEffectSource) -> Effect {
        let (r, g, b) = (0.213, 0.715, 0.072);
        // (rows are what goes in, columns what comes out: [R G B A 1] x M)
        let m = vec![
            r + (1. - r) * s, r - r * s, r - r * s, 0.,
            g - g * s, g + (1. - g) * s, g - g * s, 0.,
            b - b * s, b - b * s, b + (1. - b) * s, 0.,
            0., 0., 0., 1.,
            0., 0., 0., 0.,
        ];
        Effect { name: RefCell::new(HSTRING::from("saturate")), id: COLOR_MATRIX, props: vec![Prop::Floats(m), Prop::U(1), Prop::B(true)], sources: vec![source] }
    }
}

impl IGraphicsEffect_Impl for Effect_Impl {
    fn Name(&self) -> windows::core::Result<HSTRING> {
        Ok(self.name.borrow().clone())
    }
    fn SetName(&self, name: &HSTRING) -> windows::core::Result<()> {
        *self.name.borrow_mut() = name.clone();
        Ok(())
    }
}

impl IGraphicsEffectSource_Impl for Effect_Impl {}

impl IGraphicsEffectD2D1Interop_Impl for Effect_Impl {
    fn GetEffectId(&self) -> windows::core::Result<GUID> {
        Ok(self.id)
    }
    fn GetNamedPropertyMapping(&self, _: &PCWSTR, _: *mut u32, _: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING) -> windows::core::Result<()> {
        Err(E_INVALIDARG.into())
    }
    fn GetPropertyCount(&self) -> windows::core::Result<u32> {
        Ok(self.props.len() as u32)
    }
    fn GetProperty(&self, index: u32) -> windows::core::Result<IPropertyValue> {
        match self.props.get(index as usize) {
            Some(Prop::F(v)) => PropertyValue::CreateSingle(*v)?.cast(),
            Some(Prop::U(v)) => PropertyValue::CreateUInt32(*v)?.cast(),
            Some(Prop::B(v)) => PropertyValue::CreateBoolean(*v)?.cast(),
            Some(Prop::Floats(v)) => PropertyValue::CreateSingleArray(v)?.cast(),
            None => Err(E_INVALIDARG.into()),
        }
    }
    fn GetSource(&self, index: u32) -> windows::core::Result<IGraphicsEffectSource> {
        self.sources.get(index as usize).cloned().ok_or_else(|| E_INVALIDARG.into())
    }
    fn GetSourceCount(&self) -> windows::core::Result<u32> {
        Ok(self.sources.len() as u32)
    }
}
