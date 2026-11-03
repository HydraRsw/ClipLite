// Generador de códigos QR (ISO/IEC 18004), modo byte, corrección de errores M, versiones 1–10.
// Suficiente para la dirección de emparejamiento con el móvil. Devuelve una imagen SVG.

/// (bloques del grupo 1, datos por bloque g1, bloques g2, datos por bloque g2, corrección por bloque)
const BLOCKS_M: [(usize, usize, usize, usize, usize); 10] = [
    (1, 16, 0, 0, 10),
    (1, 28, 0, 0, 16),
    (1, 44, 0, 0, 26),
    (2, 32, 0, 0, 18),
    (2, 43, 0, 0, 24),
    (4, 27, 0, 0, 16),
    (4, 31, 0, 0, 18),
    (2, 38, 2, 39, 22),
    (3, 36, 2, 37, 22),
    (4, 43, 1, 44, 26),
];
const ALIGN: [&[usize]; 10] = [&[], &[6, 18], &[6, 22], &[6, 26], &[6, 30], &[6, 34], &[6, 22, 38], &[6, 24, 42], &[6, 26, 46], &[6, 28, 50]];

// ---- Reed-Solomon en GF(256), polinomio 0x11D
fn gf_tables() -> ([u8; 256], [u8; 256]) {
    let (mut exp, mut log) = ([0u8; 256], [0u8; 256]);
    let mut x: u16 = 1;
    for i in 0..255 {
        exp[i] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= 0x11D;
        }
    }
    exp[255] = exp[0];
    (exp, log)
}

fn rs_ec(data: &[u8], n: usize) -> Vec<u8> {
    let (exp, log) = gf_tables();
    let mul = |a: u8, b: u8| if a == 0 || b == 0 { 0 } else { exp[(log[a as usize] as usize + log[b as usize] as usize) % 255] };
    // Polinomio generador ∏ (x - α^i)
    let mut g = vec![1u8];
    for i in 0..n {
        let mut ng = vec![0u8; g.len() + 1];
        for (j, &c) in g.iter().enumerate() {
            ng[j] ^= c;
            ng[j + 1] ^= mul(c, exp[i]);
        }
        g = ng;
    }
    let mut rem = vec![0u8; n];
    for &d in data {
        let f = d ^ rem[0];
        rem.remove(0);
        rem.push(0);
        for k in 0..n {
            rem[k] ^= mul(g[k + 1], f);
        }
    }
    rem
}

fn bch(value: u32, poly: u32, bits: u32) -> u32 {
    let deg = 32 - poly.leading_zeros() - 1;
    let mut v = value << deg;
    while v >= (1 << deg) {
        let shift = (32 - v.leading_zeros()) - (deg + 1);
        v ^= poly << shift;
    }
    let _ = bits;
    (value << deg) | v
}

struct Grid {
    n: usize,
    dark: Vec<bool>,
    func: Vec<bool>,
}

impl Grid {
    fn set(&mut self, r: usize, c: usize, d: bool) {
        self.dark[r * self.n + c] = d;
        self.func[r * self.n + c] = true;
    }
    fn finder(&mut self, r0: isize, c0: isize) {
        for dr in -1..=7isize {
            for dc in -1..=7isize {
                let (r, c) = (r0 + dr, c0 + dc);
                if r < 0 || c < 0 || r >= self.n as isize || c >= self.n as isize {
                    continue;
                }
                let inside = (0..=6).contains(&dr) && (0..=6).contains(&dc);
                let d = inside && (dr == 0 || dr == 6 || dc == 0 || dc == 6 || ((2..=4).contains(&dr) && (2..=4).contains(&dc)));
                self.set(r as usize, c as usize, d);
            }
        }
    }
}

/// Matriz QR (true = módulo oscuro) para `text`, o None si no cabe en la versión 10.
pub fn encode(text: &str) -> Option<Vec<Vec<bool>>> {
    let bytes = text.as_bytes();
    let ver = (1..=10).find(|&v| {
        let (b1, d1, b2, d2, _) = BLOCKS_M[v - 1];
        let cap = b1 * d1 + b2 * d2;
        let header = 4 + if v < 10 { 8 } else { 16 };
        header + bytes.len() * 8 <= cap * 8
    })?;
    let (b1, d1, b2, d2, ecn) = BLOCKS_M[ver - 1];
    let cap = b1 * d1 + b2 * d2;

    // ---- bits de datos
    let mut bits: Vec<bool> = Vec::new();
    let mut push = |v: u32, n: u32, bits: &mut Vec<bool>| {
        for i in (0..n).rev() {
            bits.push((v >> i) & 1 == 1);
        }
    };
    push(0b0100, 4, &mut bits);
    push(bytes.len() as u32, if ver < 10 { 8 } else { 16 }, &mut bits);
    for &b in bytes {
        push(b as u32, 8, &mut bits);
    }
    let room = cap * 8 - bits.len();
    push(0, room.min(4) as u32, &mut bits);
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    let mut data: Vec<u8> = bits.chunks(8).map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b as u8)).collect();
    let mut pad = [0xEC, 0x11].iter().cycle();
    while data.len() < cap {
        data.push(*pad.next().unwrap());
    }

    // ---- bloques, corrección y entrelazado
    let mut blocks: Vec<&[u8]> = Vec::new();
    let mut off = 0;
    for (count, size) in [(b1, d1), (b2, d2)] {
        for _ in 0..count {
            blocks.push(&data[off..off + size]);
            off += size;
        }
    }
    let ecs: Vec<Vec<u8>> = blocks.iter().map(|b| rs_ec(b, ecn)).collect();
    let mut words = Vec::new();
    for i in 0..d1.max(d2) {
        for b in &blocks {
            if i < b.len() {
                words.push(b[i]);
            }
        }
    }
    for i in 0..ecn {
        for e in &ecs {
            words.push(e[i]);
        }
    }

    // ---- patrones fijos
    let n = 17 + 4 * ver;
    let mut g = Grid { n, dark: vec![false; n * n], func: vec![false; n * n] };
    g.finder(0, 0);
    g.finder(0, n as isize - 7);
    g.finder(n as isize - 7, 0);
    for i in 8..n - 8 {
        g.set(6, i, i % 2 == 0);
        g.set(i, 6, i % 2 == 0);
    }
    let al = ALIGN[ver - 1];
    for &r in al {
        for &c in al {
            if (r == 6 && c == 6) || (r == 6 && c == n - 7) || (r == n - 7 && c == 6) {
                continue;
            }
            for dr in -2..=2isize {
                for dc in -2..=2isize {
                    let d = dr.abs() == 2 || dc.abs() == 2 || (dr == 0 && dc == 0);
                    g.set((r as isize + dr) as usize, (c as isize + dc) as usize, d);
                }
            }
        }
    }
    // reservar formato (se escribe después), módulo oscuro y versión
    for i in 0..9 {
        if !g.func[8 * n + i] {
            g.set(8, i, false);
        }
        if !g.func[i * n + 8] {
            g.set(i, 8, false);
        }
    }
    for i in 0..8 {
        g.set(8, n - 1 - i, false);
        g.set(n - 1 - i, 8, false);
    }
    g.set(n - 8, 8, true); // módulo oscuro
    if ver >= 7 {
        let vi = bch(ver as u32, 0x1F25, 18);
        for i in 0..18 {
            let d = (vi >> i) & 1 == 1;
            let (a, b) = (i / 3, n - 11 + i % 3);
            g.set(a, b, d);
            g.set(b, a, d);
        }
    }

    // ---- colocar datos en zigzag
    let mut bitstream = words.iter().flat_map(|w| (0..8).rev().map(move |i| (w >> i) & 1 == 1));
    let mut col = n as isize - 1;
    let mut upward = true;
    while col > 0 {
        if col == 6 {
            col -= 1;
        }
        for k in 0..n {
            let r = if upward { n - 1 - k } else { k };
            for c in [col as usize, col as usize - 1] {
                if !g.func[r * n + c] {
                    g.dark[r * n + c] = bitstream.next().unwrap_or(false);
                }
            }
        }
        upward = !upward;
        col -= 2;
    }

    // ---- máscara con menor penalización
    let mask_fn = |m: u8, r: usize, c: usize| -> bool {
        match m {
            0 => (r + c) % 2 == 0,
            1 => r % 2 == 0,
            2 => c % 3 == 0,
            3 => (r + c) % 3 == 0,
            4 => (r / 2 + c / 3) % 2 == 0,
            5 => (r * c) % 2 + (r * c) % 3 == 0,
            6 => ((r * c) % 2 + (r * c) % 3) % 2 == 0,
            _ => ((r + c) % 2 + (r * c) % 3) % 2 == 0,
        }
    };
    let apply = |g: &Grid, m: u8| -> Vec<bool> {
        let mut d = g.dark.clone();
        for r in 0..n {
            for c in 0..n {
                if !g.func[r * n + c] && mask_fn(m, r, c) {
                    d[r * n + c] = !d[r * n + c];
                }
            }
        }
        // formato: nivel M (00) + máscara
        let fmt = bch((0b00 << 3) | m as u32, 0x537, 15) ^ 0x5412;
        let bit = |i: u32| (fmt >> i) & 1 == 1;
        for i in 0..6 {
            d[8 * n + i] = bit(14 - i as u32);
            d[i * n + 8] = bit(i as u32);
        }
        d[8 * n + 7] = bit(8);
        d[8 * n + 8] = bit(7);
        d[7 * n + 8] = bit(6);
        for i in 0..8 {
            d[8 * n + (n - 1 - i)] = bit(i as u32);
        }
        for i in 8..15 {
            d[(n - 15 + i) * n + 8] = bit(i as u32);
        }
        d[(n - 8) * n + 8] = true;
        d
    };
    let penalty = |d: &[bool]| -> u32 {
        let at = |r: usize, c: usize| d[r * n + c];
        let mut p = 0;
        for horizontal in [true, false] {
            for i in 0..n {
                let mut run = 1;
                for j in 1..n {
                    let (a, b) = if horizontal { (at(i, j), at(i, j - 1)) } else { (at(j, i), at(j - 1, i)) };
                    if a == b {
                        run += 1;
                    } else {
                        if run >= 5 {
                            p += run - 2;
                        }
                        run = 1;
                    }
                }
                if run >= 5 {
                    p += run - 2;
                }
                // patrón parecido al de localización
                for j in 0..n.saturating_sub(10) {
                    let seq: Vec<bool> = (0..11).map(|k| if horizontal { at(i, j + k) } else { at(j + k, i) }).collect();
                    let a = [true, false, true, true, true, false, true, false, false, false, false];
                    let b = [false, false, false, false, true, false, true, true, true, false, true];
                    if seq == a || seq == b {
                        p += 40;
                    }
                }
            }
        }
        for r in 0..n - 1 {
            for c in 0..n - 1 {
                let v = at(r, c);
                if at(r, c + 1) == v && at(r + 1, c) == v && at(r + 1, c + 1) == v {
                    p += 3;
                }
            }
        }
        let darkc = d.iter().filter(|&&x| x).count() as u32;
        let pct = darkc * 100 / (n * n) as u32;
        p + (pct.abs_diff(50) / 5) * 10
    };
    let forced = std::env::var("CLIPLITE_QR_MASK").ok().and_then(|v| v.parse::<u8>().ok());
    let best = match forced {
        Some(m) => apply(&g, m),
        None => (0..8u8).map(|m| apply(&g, m)).min_by_key(|d| penalty(d))?,
    };
    Some((0..n).map(|r| best[r * n..(r + 1) * n].to_vec()).collect())
}

/// Imagen SVG del código QR (con margen blanco de 4 módulos).
pub fn svg(text: &str) -> Option<String> {
    let m = encode(text)?;
    let n = m.len();
    let size = n + 8;
    let mut path = String::new();
    for (r, row) in m.iter().enumerate() {
        let mut c = 0;
        while c < n {
            if row[c] {
                let start = c;
                while c < n && row[c] {
                    c += 1;
                }
                path.push_str(&format!("M{} {}h{}v1h-{}z", start + 4, r + 4, c - start, c - start));
            } else {
                c += 1;
            }
        }
    }
    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {size} {size}\" shape-rendering=\"crispEdges\"><rect width=\"{size}\" height=\"{size}\" fill=\"#fff\"/><path d=\"{path}\" fill=\"#000\"/></svg>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_rs() {
        // Ejemplo clásico ("HELLO WORLD" 1-M): datos → corrección conocida.
        let data = [32u8, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17];
        assert_eq!(rs_ec(&data, 10), vec![196, 35, 39, 119, 235, 215, 231, 226, 93, 23]);
    }

    #[test]
    fn format_bits() {
        // Nivel M, máscara 0 → 101010000010010 (tabla del estándar).
        assert_eq!(bch(0, 0x537, 15) ^ 0x5412, 0b101010000010010);
        // Versión 7 → 000111110010010100
        assert_eq!(bch(7, 0x1F25, 18), 0b000111110010010100);
    }

    #[test]
    fn sizes() {
        assert_eq!(encode("hola").unwrap().len(), 21);
        let url = "http://192.168.100.200:47381/m?k=0123456789abcdef0123456789abcdef";
        assert_eq!(encode(url).unwrap().len(), 37); // versión 5
        assert!(encode(&"x".repeat(300)).is_none());
    }
}
