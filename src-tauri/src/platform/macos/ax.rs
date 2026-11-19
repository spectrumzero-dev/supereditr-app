//! Accessibility API bindings, and `Element`, an owned AXUIElement with typed reads and writes.

use super::text::{span_of, visible_length, with_empty_paragraphs};
use crate::platform::Rect;
use core_foundation::base::{CFGetTypeID, CFRelease, CFRetain, CFType, CFTypeID, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;

pub(super) type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;
pub(super) type AXError = i32;
pub(super) const kAXErrorSuccess: AXError = 0;
const kAXValueCGPointType: u32 = 1;
const kAXValueCGSizeType: u32 = 2;
const kAXValueCGRectType: u32 = 3;
pub(super) const kAXValueCFRangeType: u32 = 4;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub(super) struct CFRange {
    pub(super) location: isize,
    pub(super) length: isize,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub(super) struct CGSize {
    pub(super) width: f64,
    pub(super) height: f64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub(super) struct CGRect {
    origin: CGPoint,
    pub(super) size: CGSize,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub(super) fn AXIsProcessTrusted() -> bool;
    pub(super) fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
    pub(super) static kAXTrustedCheckOptionPrompt: CFStringRef;
    pub(super) fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    pub(super) fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: *mut CFTypeRef)
    -> AXError;
    fn AXUIElementSetAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: CFTypeRef) -> AXError;
    fn AXUIElementIsAttributeSettable(element: AXUIElementRef, attribute: CFStringRef, settable: *mut u8) -> AXError;
    fn AXUIElementCopyParameterizedAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        parameter: CFTypeRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> AXError;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    fn AXUIElementGetTypeID() -> CFTypeID;
    pub(super) fn AXValueCreate(kind: u32, value: *const c_void) -> AXValueRef;
    fn AXValueGetValue(value: AXValueRef, kind: u32, out: *mut c_void) -> bool;
    fn AXValueGetTypeID() -> CFTypeID;
}

/// AX timeout per message. Long enough for busy Electron apps, short enough to stay snappy.
const AX_TIMEOUT: f32 = 0.3;

/// An owned AXUIElement. AX element references are CFTypes and safe to use from any thread.
pub struct Element(pub(super) AXUIElementRef);
unsafe impl Send for Element {}
unsafe impl Sync for Element {}

impl Element {
    pub(super) fn from_create(raw: AXUIElementRef) -> Option<Self> {
        if raw.is_null() {
            return None;
        }
        unsafe { AXUIElementSetMessagingTimeout(raw, AX_TIMEOUT) };
        Some(Self(raw))
    }

    pub(super) fn attr(&self, name: &str) -> Option<CFType> {
        let name = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(self.0, name.as_concrete_TypeRef(), &mut value) };
        (err == kAXErrorSuccess && !value.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(value) })
    }

    pub(super) fn string(&self, name: &str) -> Option<String> {
        self.attr(name)?.downcast::<CFString>().map(|s| s.to_string())
    }

    pub(super) fn element(&self, name: &str) -> Option<Element> {
        let value = self.attr(name)?;
        if value.type_of() != unsafe { AXUIElementGetTypeID() } {
            return None;
        }
        let raw = value.as_CFTypeRef();
        unsafe { CFRetain(raw) };
        Element::from_create(raw)
    }

    pub(super) fn children(&self) -> Vec<Element> {
        let Some(value) = self.attr("AXChildren") else { return Vec::new() };
        let Some(array) = value.downcast::<core_foundation::array::CFArray>() else { return Vec::new() };
        array
            .iter()
            .filter(|raw| unsafe { CFGetTypeID(**raw) } == unsafe { AXUIElementGetTypeID() })
            .filter_map(|raw| {
                unsafe { CFRetain(*raw) };
                Element::from_create(*raw)
            })
            .collect()
    }

    pub(super) fn number(&self, name: &str) -> Option<i64> {
        self.attr(name)?.downcast::<core_foundation::number::CFNumber>()?.to_i64()
    }

    pub(super) fn flag(&self, name: &str) -> Option<bool> {
        self.attr(name)?.downcast::<CFBoolean>().map(bool::from)
    }

    /// Whether a flag is on, for an app that may report it as a number.
    pub(super) fn is_on(&self, name: &str) -> bool {
        self.flag(name).or_else(|| self.number(name).map(|n| n != 0)).unwrap_or(false)
    }

    pub(super) fn press(&self) -> bool {
        let action = CFString::new("AXPress");
        unsafe { AXUIElementPerformAction(self.0, action.as_concrete_TypeRef()) == kAXErrorSuccess }
    }

    pub(super) fn range(&self, name: &str) -> Option<CFRange> {
        let value = self.attr(name)?;
        ax_value(value.as_CFTypeRef(), kAXValueCFRangeType)
    }

    pub(super) fn settable(&self, name: &str) -> bool {
        let name = CFString::new(name);
        let mut settable = 0u8;
        let err = unsafe { AXUIElementIsAttributeSettable(self.0, name.as_concrete_TypeRef(), &mut settable) };
        err == kAXErrorSuccess && settable != 0
    }

    pub(super) fn set(&self, name: &str, value: CFTypeRef) -> bool {
        let name = CFString::new(name);
        unsafe { AXUIElementSetAttributeValue(self.0, name.as_concrete_TypeRef(), value) == kAXErrorSuccess }
    }

    fn parameterized(&self, name: &str, param: CFTypeRef) -> Option<CFType> {
        let name = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = unsafe {
            AXUIElementCopyParameterizedAttributeValue(self.0, name.as_concrete_TypeRef(), param, &mut value)
        };
        (err == kAXErrorSuccess && !value.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(value) })
    }

    pub(super) fn bounds_for_range(&self, range: CFRange) -> Option<CGRect> {
        let param = unsafe { AXValueCreate(kAXValueCFRangeType, &range as *const CFRange as *const c_void) };
        if param.is_null() {
            return None;
        }
        let param = unsafe { CFType::wrap_under_create_rule(param) };
        let value = self.parameterized("AXBoundsForRange", param.as_CFTypeRef())?;
        ax_value(value.as_CFTypeRef(), kAXValueCGRectType)
    }

    /// The selection in WebKit and Chromium content that only exposes it as a text-marker range.
    pub(super) fn marker_selection(&self) -> Option<(String, Option<CGRect>)> {
        let range = self.attr("AXSelectedTextMarkerRange")?;
        let text =
            self.parameterized("AXStringForTextMarkerRange", range.as_CFTypeRef())?.downcast::<CFString>()?.to_string();
        let bounds = self
            .parameterized("AXBoundsForTextMarkerRange", range.as_CFTypeRef())
            .and_then(|v| ax_value::<CGRect>(v.as_CFTypeRef(), kAXValueCGRectType));
        Some((text, bounds))
    }

    /// Selected text through whichever interface the element supports.
    /// The selected text, unless nothing but whitespace is selected: some apps report a caret
    /// sitting in a field as a selected line break or space.
    pub(super) fn selected_text(&self) -> Option<String> {
        let real = |s: &String| !s.trim().is_empty();
        self.string("AXSelectedText")
            .filter(real)
            .map(|text| self.web_text().and_then(|full| span_of(&full, &text)).unwrap_or(text))
            .or_else(|| self.marker_selection().map(|(text, _)| text).filter(real))
    }

    /// The text of a Chromium rich field (a text area whose children are its blocks: paragraphs,
    /// headings, lists) with every line break. Chromium's own text drops some: an empty paragraph
    /// adds nothing to `AXValue`, and `AXSelectedText` can leave out the break between two blocks or
    /// list items. Each block is still a child, an empty paragraph included, so the empty ones are
    /// put back. None for any other element, before its value is read.
    pub(super) fn web_text(&self) -> Option<String> {
        let paragraphs = self.children();
        if paragraphs.is_empty() || paragraphs.len() > 500 || self.string("AXRole").as_deref() != Some("AXTextArea") {
            return None;
        }
        let lengths: Vec<usize> = paragraphs.iter().map(|p| visible_length(p, 0)).collect();
        let value = self.string("AXValue")?;
        Some(with_empty_paragraphs(&value, &lengths).unwrap_or(value))
    }

    pub(super) fn frame(&self) -> Option<CGRect> {
        let position: CGPoint = ax_value(self.attr("AXPosition")?.as_CFTypeRef(), kAXValueCGPointType)?;
        let size: CGSize = ax_value(self.attr("AXSize")?.as_CFTypeRef(), kAXValueCGSizeType)?;
        Some(CGRect { origin: position, size })
    }

    pub(super) fn pid(&self) -> Option<i32> {
        let mut pid = 0;
        (unsafe { AXUIElementGetPid(self.0, &mut pid) } == kAXErrorSuccess).then_some(pid)
    }

    pub(super) fn same_as(&self, other: &Element) -> bool {
        unsafe { core_foundation::base::CFEqual(self.0, other.0) != 0 }
    }
}

impl Clone for Element {
    fn clone(&self) -> Self {
        unsafe { CFRetain(self.0) };
        Self(self.0)
    }
}

impl Drop for Element {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

impl std::fmt::Debug for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AXUIElement")
    }
}

fn ax_value<T: Default>(raw: CFTypeRef, kind: u32) -> Option<T> {
    if raw.is_null() || unsafe { CFGetTypeID(raw) } != unsafe { AXValueGetTypeID() } {
        return None;
    }
    let mut out = T::default();
    unsafe { AXValueGetValue(raw, kind, &mut out as *mut T as *mut c_void) }.then_some(out)
}

pub(super) fn to_rect(r: CGRect) -> Rect {
    Rect { x: r.origin.x, y: r.origin.y, width: r.size.width, height: r.size.height }
}
