//! Parser for generic signatures (`dalvik.annotation.Signature` / JVM `Signature` attribute).

use crate::model::types::{ArgType, WildcardBound};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SignatureError(pub String);

type Res<T> = Result<T, SignatureError>;

const STOP: char = '\0';

pub struct SignatureParser {
    sign: Vec<char>,
    pos: isize,
    mark: isize,
}

impl SignatureParser {
    pub fn new(signature: &str) -> Self {
        SignatureParser {
            sign: signature.chars().collect(),
            pos: -1,
            mark: 0,
        }
    }

    fn end(&self) -> isize {
        self.sign.len() as isize
    }

    fn next(&mut self) -> char {
        self.pos += 1;
        if self.pos >= self.end() {
            return STOP;
        }
        self.sign[self.pos as usize]
    }

    fn look_ahead(&self, ch: char) -> bool {
        let n = self.pos + 1;
        n < self.end() && self.sign[n as usize] == ch
    }

    fn mark(&mut self) {
        self.mark = self.pos;
    }

    /// Exclusive slice: from mark to the current position (current char not included).
    fn slice(&self) -> String {
        let start = if self.mark == -1 { 0 } else { self.mark };
        if start >= self.pos {
            return String::new();
        }
        self.sign[start as usize..self.pos as usize]
            .iter()
            .collect()
    }

    fn inclusive_slice(&self) -> String {
        let start = if self.mark == -1 { 0 } else { self.mark };
        let last = self.pos + 1;
        if start >= last {
            return String::new();
        }
        self.sign[start as usize..(last as usize).min(self.sign.len())]
            .iter()
            .collect()
    }

    fn skip_until(&mut self, until: char) -> bool {
        let start = self.pos;
        loop {
            if self.look_ahead(until) {
                return true;
            }
            if self.next() == STOP {
                self.pos = start;
                return false;
            }
        }
    }

    fn debug(&self) -> String {
        let s: String = self.sign.iter().collect();
        format!("{s} at position {}", self.pos)
    }

    fn consume(&mut self, exp: char) -> Res<()> {
        let c = self.next();
        if c != exp {
            return Err(SignatureError(format!(
                "Consume wrong char: '{c}' != '{exp}', sign: {}",
                self.debug()
            )));
        }
        Ok(())
    }

    fn try_consume(&mut self, exp: char) -> bool {
        if self.look_ahead(exp) {
            self.next();
            return true;
        }
        false
    }

    fn consume_until(&mut self, last: char) -> Option<String> {
        self.mark();
        if self.skip_until(last) {
            Some(self.inclusive_slice())
        } else {
            None
        }
    }

    pub fn consume_type(&mut self) -> Res<Option<ArgType>> {
        let ch = self.next();
        match ch {
            'L' => {
                if let Some(obj) = self.consume_object_type(false)? {
                    return Ok(Some(obj));
                }
            }
            'T' => {
                self.next();
                self.mark();
                if let Some(name) = self.consume_until(';') {
                    self.consume(';')?;
                    if name.contains(')') {
                        return Err(SignatureError(format!(
                            "Bad name for type variable: {name}"
                        )));
                    }
                    return Ok(Some(ArgType::generic_type(&name)));
                }
            }
            '[' => {
                let inner = self.consume_type()?;
                return match inner {
                    Some(t) => Ok(Some(ArgType::array(t))),
                    None => Err(SignatureError("array without element type".into())),
                };
            }
            STOP => return Ok(None),
            c => {
                if let Ok(t) = ArgType::parse_char(c) {
                    return Ok(Some(t));
                }
                return Err(SignatureError(format!("Unknown type char: '{c}'")));
            }
        }
        Err(SignatureError(format!(
            "Can't parse type: {}, unexpected: {ch}",
            self.debug()
        )))
    }

    pub fn consume_type_list(&mut self) -> Res<Vec<ArgType>> {
        let mut list = Vec::new();
        while let Some(t) = self.consume_type()? {
            list.push(t);
        }
        Ok(list)
    }

    fn consume_object_type(&mut self, inner_type: bool) -> Res<Option<ArgType>> {
        self.mark();
        let mut ch;
        loop {
            if inner_type && self.look_ahead('.') {
                return Ok(Some(ArgType::object(&self.inclusive_slice())));
            }
            ch = self.next();
            if ch == STOP {
                return Ok(None);
            }
            if ch == '<' || ch == ';' {
                break;
            }
        }
        if ch == ';' {
            let obj = if inner_type {
                self.slice().replace('/', ".")
            } else {
                self.inclusive_slice()
            };
            return Ok(Some(ArgType::object(&obj)));
        }
        let mut obj = self.slice();
        if !inner_type {
            obj.push(';');
        } else {
            obj = obj.replace('/', ".");
        }
        let type_vars = self.consume_generic_args()?;
        self.consume('>')?;
        let mut generic = ArgType::generic_named(&obj, type_vars);
        if !self.look_ahead('.') {
            self.consume(';')?;
            return Ok(Some(generic));
        }
        self.consume('.')?;
        self.next();
        let mut inner = self
            .consume_object_type(true)?
            .ok_or_else(|| SignatureError(format!("No inner type found: {}", self.debug())))?;
        while self.look_ahead('.') {
            generic = ArgType::OuterGeneric {
                outer: Box::new(generic),
                inner: Box::new(inner),
            };
            self.consume('.')?;
            self.next();
            inner = self.consume_object_type(true)?.ok_or_else(|| {
                SignatureError(format!("Unexpected inner type found: {}", self.debug()))
            })?;
        }
        Ok(Some(ArgType::OuterGeneric {
            outer: Box::new(generic),
            inner: Box::new(inner),
        }))
    }

    fn consume_generic_args(&mut self) -> Res<Vec<ArgType>> {
        let mut list = Vec::new();
        loop {
            let ty = if self.look_ahead('*') {
                self.next();
                Some(ArgType::wildcard_unbound())
            } else if self.look_ahead('+') {
                self.next();
                let t = self
                    .consume_type()?
                    .ok_or_else(|| SignatureError("missing wildcard type".into()))?;
                Some(ArgType::Wildcard {
                    ty: Box::new(t),
                    bound: WildcardBound::Extends,
                })
            } else if self.look_ahead('-') {
                self.next();
                let t = self
                    .consume_type()?
                    .ok_or_else(|| SignatureError("missing wildcard type".into()))?;
                Some(ArgType::Wildcard {
                    ty: Box::new(t),
                    bound: WildcardBound::Super,
                })
            } else {
                self.consume_type()?
            };
            match ty {
                Some(t) => list.push(t),
                None => break,
            }
            if self.look_ahead('>') {
                break;
            }
        }
        Ok(list)
    }

    /// `<T:Ljava/lang/Exception;:Ljava/lang/Object;>`: type variables with their bounds.
    pub fn consume_generic_type_parameters(&mut self) -> Res<Vec<ArgType>> {
        if !self.look_ahead('<') {
            return Ok(Vec::new());
        }
        let mut list = Vec::new();
        self.consume('<')?;
        loop {
            if self.look_ahead('>') || self.next() == STOP {
                break;
            }
            let id = self
                .consume_until(':')
                .ok_or_else(|| SignatureError("Failed to parse generic types map".into()))?;
            self.consume(':')?;
            self.try_consume(':');
            let types = self.consume_extends_types_list()?;
            list.push(ArgType::GenericType {
                name: id,
                extend_types: types,
            });
        }
        self.consume('>')?;
        Ok(list)
    }

    fn consume_extends_types_list(&mut self) -> Res<Vec<ArgType>> {
        let mut types = Vec::new();
        loop {
            let t = self
                .consume_type()?
                .ok_or_else(|| SignatureError("Unexpected end of signature".into()))?;
            if t != ArgType::object_type() {
                types.push(t);
            }
            if self.look_ahead(':') {
                self.consume(':')?;
            } else {
                break;
            }
        }
        Ok(types)
    }

    pub fn consume_method_args(&mut self, args_count: usize) -> Res<Vec<ArgType>> {
        self.consume('(')?;
        if self.look_ahead(')') {
            self.consume(')')?;
            return Ok(Vec::new());
        }
        let mut args = Vec::new();
        let limit = args_count + 10;
        loop {
            let t = self
                .consume_type()?
                .ok_or_else(|| SignatureError("Unexpected end of signature".into()))?;
            args.push(t);
            if args.len() > limit {
                return Err(SignatureError(format!(
                    "Arguments count limit reached: {}",
                    args.len()
                )));
            }
            if self.look_ahead(')') {
                break;
            }
        }
        self.consume(')')?;
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_signature() {
        let mut p = SignatureParser::new("<T:Ljava/lang/Number;>(Ljava/util/List<TT;>;[I)TT;");
        let tp = p.consume_generic_type_parameters().unwrap();
        assert_eq!(tp[0].to_string(), "T extends java.lang.Number");
        let args = p.consume_method_args(2).unwrap();
        assert_eq!(args[0].to_string(), "java.util.List<T>");
        assert_eq!(args[1].to_string(), "int[]");
        assert_eq!(p.consume_type().unwrap().unwrap().to_string(), "T");
    }

    #[test]
    fn wildcards_and_inner() {
        let mut p = SignatureParser::new("Ljava/util/Map<+Ljava/lang/String;*>.Entry<TK;>;");
        let t = p.consume_type().unwrap().unwrap();
        assert_eq!(
            t.to_string(),
            "java.util.Map<? extends java.lang.String, ?>$Entry<K>"
        );
    }

    #[test]
    fn class_signature() {
        let mut p = SignatureParser::new(
            "<E:Ljava/lang/Object;>Ljava/util/AbstractList<TE;>;Ljava/util/List<TE;>;",
        );
        let g = p.consume_generic_type_parameters().unwrap();
        assert_eq!(g[0].to_string(), "E");
        let sup = p.consume_type().unwrap().unwrap();
        assert_eq!(sup.to_string(), "java.util.AbstractList<E>");
        assert_eq!(p.consume_type_list().unwrap().len(), 1);
    }
}
