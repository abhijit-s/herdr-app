#![allow(unused, non_upper_case_globals)]

use cocoa::appkit::CGFloat;
use core_foundation::{
    array::{
        CFArray, CFArrayAppendArray, CFArrayAppendValue, CFArrayCreateMutable, CFArrayGetCount,
        CFArrayGetValueAtIndex, CFArrayRef, CFMutableArrayRef, kCFTypeArrayCallBacks,
    },
    base::{CFRelease, TCFType, kCFAllocatorDefault},
    dictionary::{
        CFDictionaryCreate, kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks,
    },
    number::CFNumber,
    string::{CFString, CFStringRef},
};
use core_foundation_sys::locale::CFLocaleCopyPreferredLanguages;
use core_graphics::{display::CFDictionary, geometry::CGAffineTransform};
use core_text::font_descriptor::{
    TraitAccessors, kCTFontFamilyNameAttribute, kCTFontItalicTrait, kCTFontSlantTrait,
    kCTFontTraitsAttribute, kCTFontWeightTrait, kCTFontWidthTrait,
};
use core_text::{
    font::{CTFont, CTFontRef, cascade_list_for_languages},
    font_descriptor::{
        CTFontDescriptor, CTFontDescriptorCopyAttributes, CTFontDescriptorCreateCopyWithFeature,
        CTFontDescriptorCreateWithAttributes, CTFontDescriptorCreateWithNameAndSize,
        CTFontDescriptorRef, kCTFontCascadeListAttribute, kCTFontFeatureSettingsAttribute,
    },
};
use font_kit::font::Font as FontKitFont;
use gpui::{FontFallbacks, FontFeatures};
use std::ptr;

/// HERDR: `weight` (CSS 100..900), when given, is the weight fallbacks are
/// asked for, in place of the face's own: a variable face drawn at another
/// weight (`at_weight`) reports the weight it was loaded at.
pub fn apply_features_and_fallbacks(
    font: &mut FontKitFont,
    features: &FontFeatures,
    fallbacks: Option<&FontFallbacks>,
    weight: Option<f32>,
) -> anyhow::Result<()> {
    unsafe {
        let mut keys = vec![kCTFontFeatureSettingsAttribute];
        let mut values = vec![generate_feature_array(features)];
        if let Some(fallbacks) = fallbacks
            && !fallbacks.fallback_list().is_empty()
        {
            keys.push(kCTFontCascadeListAttribute);
            values.push(generate_fallback_array(fallbacks, font, weight));
        }
        let attrs = CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr() as _,
            values.as_ptr() as _,
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );

        for value in &values {
            CFRelease(*value as _);
        }

        let new_descriptor = CTFontDescriptorCreateWithAttributes(attrs);
        CFRelease(attrs as _);
        let new_descriptor = CTFontDescriptor::wrap_under_create_rule(new_descriptor);
        let new_font = CTFontCreateCopyWithAttributes(
            font.native_font().as_concrete_TypeRef(),
            0.0,
            std::ptr::null(),
            new_descriptor.as_concrete_TypeRef(),
        );
        let new_font = CTFont::wrap_under_create_rule(new_font);
        *font = font_kit::font::Font::from_native_font(&new_font);

        Ok(())
    }
}

fn generate_feature_array(features: &FontFeatures) -> CFMutableArrayRef {
    unsafe {
        let feature_array = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
        for (tag, value) in features.tag_value_list() {
            let keys = [kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue];
            let values = [
                CFString::new(tag).as_CFTypeRef(),
                CFNumber::from(*value as i32).as_CFTypeRef(),
            ];
            let dict = CFDictionaryCreate(
                kCFAllocatorDefault,
                &keys as *const _ as _,
                &values as *const _ as _,
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            values.into_iter().for_each(|value| CFRelease(value));
            CFArrayAppendValue(feature_array, dict as _);
            CFRelease(dict as _);
        }
        feature_array
    }
}

/// HERDR: a CSS weight on CoreText's normalized weight trait scale (-1..1),
/// the mapping font-kit reads CoreText's traits back with: 400 is 0, 700 is 0.4.
pub fn normalized_weight(css: f32) -> f64 {
    const TRAITS: [f32; 9] = [-0.7, -0.5, -0.23, 0.0, 0.2, 0.3, 0.4, 0.6, 0.8];
    let at = (css.clamp(100.0, 900.0) - 100.0) / 100.0;
    let (lo, frac) = (at.floor() as usize, at.fract());
    let hi = (lo + 1).min(TRAITS.len() - 1);
    f64::from(TRAITS[lo] + (TRAITS[hi] - TRAITS[lo]) * frac)
}

fn generate_fallback_array(fallbacks: &FontFallbacks, font: &mut FontKitFont, weight: Option<f32>) -> CFMutableArrayRef {
    unsafe {
        let symbolic_traits = font.native_font().symbolic_traits();
        let all_traits = font.native_font().all_traits();

        let fallback_array = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
        for user_fallback in fallbacks.fallback_list() {
            let name = CFString::from(user_fallback.as_str());

            let traits_keys = [kCTFontWeightTrait, kCTFontSlantTrait];
            let weight_value = CFNumber::from(weight.map_or_else(|| all_traits.normalized_weight(), normalized_weight));
            let slant_value = CFNumber::from(if (symbolic_traits & kCTFontItalicTrait) != 0 {
                1.0
            } else {
                0.0
            });
            let traits_values = [weight_value.as_CFTypeRef(), slant_value.as_CFTypeRef()];
            let traits = CFDictionaryCreate(
                kCFAllocatorDefault,
                &traits_keys as *const _ as _,
                &traits_values as *const _ as _,
                traits_keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            drop(weight_value);
            drop(slant_value);

            let attr_keys = [kCTFontFamilyNameAttribute, kCTFontTraitsAttribute];
            let attr_values = [name.as_CFTypeRef(), traits as _];
            let attrs = CFDictionaryCreate(
                kCFAllocatorDefault,
                &attr_keys as *const _ as _,
                &attr_values as *const _ as _,
                attr_keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(traits as _);

            let fallback_desc = CTFontDescriptorCreateWithAttributes(attrs);
            CFRelease(attrs as _);

            CFArrayAppendValue(fallback_array, fallback_desc as _);
            CFRelease(fallback_desc as _);
        }

        let font_ref = font.native_font().as_concrete_TypeRef();
        append_system_fallbacks(fallback_array, font_ref);
        fallback_array
    }
}

fn append_system_fallbacks(fallback_array: CFMutableArrayRef, font_ref: CTFontRef) {
    unsafe {
        let preferred_languages: CFArray<CFString> =
            CFArray::wrap_under_create_rule(CFLocaleCopyPreferredLanguages());

        let default_fallbacks = CTFontCopyDefaultCascadeListForLanguages(
            font_ref,
            preferred_languages.as_concrete_TypeRef(),
        );
        let default_fallbacks: CFArray<CTFontDescriptor> =
            CFArray::wrap_under_create_rule(default_fallbacks);

        for desc in default_fallbacks
            .iter()
            .filter(|desc| desc.font_path().is_some())
        {
            CFArrayAppendValue(fallback_array, desc.as_concrete_TypeRef() as _);
        }
    }
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    static kCTFontOpenTypeFeatureTag: CFStringRef;
    static kCTFontOpenTypeFeatureValue: CFStringRef;

    fn CTFontCreateCopyWithAttributes(
        font: CTFontRef,
        size: CGFloat,
        matrix: *const CGAffineTransform,
        attributes: CTFontDescriptorRef,
    ) -> CTFontRef;
    fn CTFontCopyDefaultCascadeListForLanguages(
        font: CTFontRef,
        languagePrefList: CFArrayRef,
    ) -> CFArrayRef;
}

#[cfg(test)]
mod tests {
    #[test]
    fn css_weights_map_onto_coretexts_weight_trait() {
        for (css, normalized) in [(100.0, -0.7), (400.0, 0.0), (450.0, 0.1), (700.0, 0.4), (900.0, 0.8), (50.0, -0.7), (1000.0, 0.8)] {
            assert!((super::normalized_weight(css) - normalized).abs() < 1e-6, "{css}");
        }
    }
}
