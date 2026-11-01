// Analizador JSON mínimo (solo lo necesario para las APIs locales de los juegos).

#[derive(Debug, Clone)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn path(&self, keys: &[&str]) -> Option<&Json> {
        keys.iter().try_fold(self, |j, k| j.get(k))
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            Json::Str(s) => s.parse().ok(),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> &[Json] {
        match self {
            Json::Arr(v) => v,
            _ => &[],
        }
    }
}

pub fn parse(s: &str) -> Option<Json> {
    let mut p = P { b: s.as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    Some(v)
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> P<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn lit(&mut self, word: &str, v: Json) -> Option<Json> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Some(v)
        } else {
            None
        }
    }
    fn value(&mut self, depth: u32) -> Option<Json> {
        if depth > 64 {
            return None;
        }
        self.ws();
        match self.peek()? {
            b'{' => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.peek()? == b'}' {
                    self.i += 1;
                    return Some(Json::Obj(v));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    if self.peek()? != b':' {
                        return None;
                    }
                    self.i += 1;
                    let val = self.value(depth + 1)?;
                    v.push((k, val));
                    self.ws();
                    match self.peek()? {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            return Some(Json::Obj(v));
                        }
                        _ => return None,
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.peek()? == b']' {
                    self.i += 1;
                    return Some(Json::Arr(v));
                }
                loop {
                    v.push(self.value(depth + 1)?);
                    self.ws();
                    match self.peek()? {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Some(Json::Arr(v));
                        }
                        _ => return None,
                    }
                }
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.lit("true", Json::Bool(true)),
            b'f' => self.lit("false", Json::Bool(false)),
            b'n' => self.lit("null", Json::Null),
            _ => {
                let start = self.i;
                while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[start..self.i]).ok()?.parse().ok().map(Json::Num)
            }
        }
    }
    fn hex4(&mut self) -> Option<u32> {
        let s = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(s, 16).ok()
    }
    fn string(&mut self) -> Option<String> {
        if self.peek()? != b'"' {
            return None;
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.b.len() && self.b[self.i] != b'"' && self.b[self.i] != b'\\' {
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.b[start..self.i]).ok()?);
            match self.peek()? {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                _ => {
                    self.i += 1;
                    let e = self.peek()?;
                    self.i += 1;
                    match e {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let mut c = self.hex4()?;
                            if (0xD800..0xDC00).contains(&c) && self.b.get(self.i..self.i + 2) == Some(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                c = 0x10000 + ((c - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            out.push(char::from_u32(c).unwrap_or('?'));
                        }
                        other => out.push(other as char),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_lol_like() {
        let j = super::parse(r#"{"Events":[{"EventID":3,"KillerName":"Ñandú \"x\"","Assisters":["a","b"],"t":-1.5e2,"ok":true,"n":null}],"e":"\ud83d\ude00"}"#).unwrap();
        let ev = &j.get("Events").unwrap().as_arr()[0];
        assert_eq!(ev.get("EventID").unwrap().as_f64(), Some(3.0));
        assert_eq!(ev.get("KillerName").unwrap().as_str(), Some("Ñandú \"x\""));
        assert_eq!(ev.get("Assisters").unwrap().as_arr().len(), 2);
        assert_eq!(ev.get("t").unwrap().as_f64(), Some(-150.0));
        assert_eq!(j.get("e").unwrap().as_str(), Some("😀"));
        assert!(super::parse("{\"a\":").is_none());
    }
}
