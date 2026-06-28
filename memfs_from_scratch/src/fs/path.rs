pub fn split_parent(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return ("/", "");
    }
    match trimmed.rfind('/') {
        Some(0) => ("/", &trimmed[1..]),
        Some(pos) => (&trimmed[..pos], &trimmed[pos + 1..]),
        None => (".", trimmed),
    }
}

pub struct PathCursor<'a> {
    rest: &'a str,
}

impl<'a> PathCursor<'a> {
    pub fn new(path: &'a str) -> Self {
        Self { rest: path }
    }
}

impl<'a> Iterator for PathCursor<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            self.rest = self.rest.trim_start_matches('/');
            if self.rest.is_empty() {
                return None;
            }
            match self.rest.find('/') {
                Some(pos) => {
                    let part = &self.rest[..pos];
                    self.rest = &self.rest[pos + 1..];
                    if part == "." || part.is_empty() {
                        continue;
                    }
                    return Some(part);
                }
                None => {
                    let part = self.rest;
                    self.rest = "";
                    if part == "." || part.is_empty() {
                        continue;
                    }
                    return Some(part);
                }
            }
        }
    }
}
