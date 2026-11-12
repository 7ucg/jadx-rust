//! Access flags with the modifier text of the reference (`AccessInfo`).

pub const ACC_PUBLIC: u32 = 0x0001;
pub const ACC_PRIVATE: u32 = 0x0002;
pub const ACC_PROTECTED: u32 = 0x0004;
pub const ACC_STATIC: u32 = 0x0008;
pub const ACC_FINAL: u32 = 0x0010;
pub const ACC_SYNCHRONIZED: u32 = 0x0020;
pub const ACC_VOLATILE: u32 = 0x0040;
pub const ACC_BRIDGE: u32 = 0x0040;
pub const ACC_TRANSIENT: u32 = 0x0080;
pub const ACC_VARARGS: u32 = 0x0080;
pub const ACC_NATIVE: u32 = 0x0100;
pub const ACC_INTERFACE: u32 = 0x0200;
pub const ACC_ABSTRACT: u32 = 0x0400;
pub const ACC_STRICT: u32 = 0x0800;
pub const ACC_SYNTHETIC: u32 = 0x1000;
pub const ACC_ANNOTATION: u32 = 0x2000;
pub const ACC_ENUM: u32 = 0x4000;

/// What the flags belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessKind {
    Class,
    Field,
    Method,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessInfo {
    pub flags: u32,
    pub kind: AccessKind,
}

impl AccessInfo {
    pub fn new(flags: u32, kind: AccessKind) -> Self {
        AccessInfo { flags, kind }
    }

    fn has(&self, f: u32) -> bool {
        self.flags & f != 0
    }

    pub fn is_public(&self) -> bool {
        self.has(ACC_PUBLIC)
    }
    pub fn is_private(&self) -> bool {
        self.has(ACC_PRIVATE)
    }
    pub fn is_protected(&self) -> bool {
        self.has(ACC_PROTECTED)
    }
    pub fn is_static(&self) -> bool {
        self.has(ACC_STATIC)
    }
    pub fn is_final(&self) -> bool {
        self.has(ACC_FINAL)
    }
    pub fn is_abstract(&self) -> bool {
        self.has(ACC_ABSTRACT)
    }
    pub fn is_native(&self) -> bool {
        self.has(ACC_NATIVE)
    }
    pub fn is_interface(&self) -> bool {
        self.has(ACC_INTERFACE)
    }
    pub fn is_annotation(&self) -> bool {
        self.has(ACC_ANNOTATION)
    }
    pub fn is_enum(&self) -> bool {
        self.has(ACC_ENUM)
    }
    pub fn is_synthetic(&self) -> bool {
        self.has(ACC_SYNTHETIC)
    }

    /// Same flags with `flags` cleared.
    pub fn remove(self, flags: u32) -> Self {
        AccessInfo {
            flags: self.flags & !flags,
            kind: self.kind,
        }
    }

    /// Modifier text with a trailing space after every modifier.
    pub fn make_string(&self, show_hidden: bool) -> String {
        let mut s = String::new();
        if self.is_public() {
            s.push_str("public ");
        }
        if self.is_private() {
            s.push_str("private ");
        }
        if self.is_protected() {
            s.push_str("protected ");
        }
        if self.is_static() {
            s.push_str("static ");
        }
        if self.is_final() {
            s.push_str("final ");
        }
        if self.is_abstract() {
            s.push_str("abstract ");
        }
        if self.is_native() {
            s.push_str("native ");
        }
        match self.kind {
            AccessKind::Method => {
                if self.has(ACC_SYNCHRONIZED) {
                    s.push_str("synchronized ");
                }
                if show_hidden && self.has(ACC_BRIDGE) {
                    s.push_str("/* bridge */ ");
                }
            }
            AccessKind::Field => {
                if self.has(ACC_VOLATILE) {
                    s.push_str("volatile ");
                }
                if self.has(ACC_TRANSIENT) {
                    s.push_str("transient ");
                }
            }
            AccessKind::Class => {
                if self.has(ACC_STRICT) {
                    s.push_str("strict ");
                }
            }
        }
        if self.is_synthetic() && show_hidden {
            s.push_str("/* synthetic */ ");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_order() {
        let a = AccessInfo::new(
            ACC_PUBLIC | ACC_STATIC | ACC_FINAL | ACC_VOLATILE,
            AccessKind::Field,
        );
        assert_eq!(a.make_string(false), "public static final volatile ");
        let m = AccessInfo::new(
            ACC_PUBLIC | ACC_SYNCHRONIZED | ACC_BRIDGE,
            AccessKind::Method,
        );
        assert_eq!(m.make_string(true), "public synchronized /* bridge */ ");
        assert_eq!(m.make_string(false), "public synchronized ");
    }
}
