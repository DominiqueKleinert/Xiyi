// src/semantic/privacy.rs
use crate::ast::*;
use super::check_program::TypeChecker;

impl TypeChecker {
    pub fn types_equal_with_privacy(&self, a: &Type, b: &Type) -> bool {
        match (a, b) {
            (Type::Privacy(inner1, tag1), Type::Privacy(inner2, tag2)) => {
                self.types_equal(inner1, inner2) && self.privacy_tags_equal(tag1, tag2)
            }
            (Type::Privacy(inner, _), other) => self.types_equal(inner, other),
            (other, Type::Privacy(inner, _)) => self.types_equal(other, inner),
            _ => self.types_equal(a, b),
        }
    }

    pub fn privacy_tags_equal(&self, a: &PrivacyTag, b: &PrivacyTag) -> bool {
        match (a, b) {
            (PrivacyTag::Public, PrivacyTag::Public) => true,
            (PrivacyTag::Private, PrivacyTag::Private) => true,
            (PrivacyTag::Differential { eps: e1, delta: d1 }, PrivacyTag::Differential { eps: e2, delta: d2 }) => {
                Self::rational_eq(e1, e2)
                    && match (d1, d2) {
                        (Some(d1), Some(d2)) => Self::rational_eq(d1, d2),
                        (None, None) => true,
                        _ => false,
                    }
            }
            _ => false,
        }
    }

    pub fn join_privacy_tags(&self, a: &PrivacyTag, b: &PrivacyTag) -> PrivacyTag {
        match (a, b) {
            (PrivacyTag::Public, x) => x.clone(),
            (x, PrivacyTag::Public) => x.clone(),
            (PrivacyTag::Private, _) => PrivacyTag::Private,
            (_, PrivacyTag::Private) => PrivacyTag::Private,
            (PrivacyTag::Differential { eps: e1, delta: d1 }, PrivacyTag::Differential { eps: e2, delta: d2 }) => {
                let eps = Self::rational_min(e1, e2);
                let delta = match (d1, d2) {
                    (Some(d1), Some(d2)) => {
                        if Self::rational_le(d1, d2) { Some(d2.clone()) } else { Some(d1.clone()) }
                    }
                    (Some(d), None) | (None, Some(d)) => Some(d.clone()),
                    (None, None) => None,
                };
                PrivacyTag::Differential { eps, delta }
            }
        }
    }

    pub fn extract_privacy_tag(&self, ty: &Type) -> Option<PrivacyTag> {
        match ty {
            Type::Privacy(_, tag) => Some(tag.clone()),
            Type::Ref { inner, .. } => self.extract_privacy_tag(inner),
            _ => None,
        }
    }

    pub fn apply_privacy_tag(&self, ty: Type, tag: Option<PrivacyTag>) -> Type {
        match tag {
            Some(t) => Type::Privacy(Box::new(ty), t),
            None => ty,
        }
    }

    pub fn join_privacy_labels(&self, a: &Type, b: &Type) -> Option<PrivacyTag> {
        let tag_a = self.extract_privacy_tag(a);
        let tag_b = self.extract_privacy_tag(b);
        match (tag_a, tag_b) {
            (Some(t1), Some(t2)) => Some(self.join_privacy_tags(&t1, &t2)),
            (Some(t), None) => Some(t),
            (None, Some(t)) => Some(t),
            (None, None) => None,
        }
    }

    pub fn strip_privacy(&self, ty: &Type) -> Type {
        match ty {
            Type::Privacy(inner, _) => self.strip_privacy(inner),
            Type::Ref { mutable, inner } => Type::Ref {
                mutable: *mutable,
                inner: Box::new(self.strip_privacy(inner)),
            },
            _ => ty.clone(),
        }
    }
}
