use anyhow::{bail, Result};

pub fn evaluate_condition(
    condition: &str,
    edges: &[(String, String)],
    variants: &[String],
) -> Result<bool> {
    let mut parser = Parser {
        remaining: condition.trim(),
        edges,
        variants,
    };
    let value = parser.parse_or()?;
    if !parser.remaining.trim().is_empty() {
        bail!("Unexpected text: {}", parser.remaining.trim());
    }
    Ok(value)
}

struct Parser<'a> {
    remaining: &'a str,
    edges: &'a [(String, String)],
    variants: &'a [String],
}

impl<'a> Parser<'a> {
    fn consume(&mut self, token: &str) -> bool {
        self.remaining = self.remaining.trim_start();
        if let Some(rest) = self.remaining.strip_prefix(token) {
            self.remaining = rest;
            true
        } else {
            false
        }
    }

    fn parse_name(&mut self) -> Result<&'a str> {
        self.remaining = self.remaining.trim_start();
        let end = self
            .remaining
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .unwrap_or(self.remaining.len());
        if end == 0 {
            bail!("Expected a slot or variant name");
        }
        let name = &self.remaining[..end];
        self.remaining = &self.remaining[end..];
        Ok(name)
    }

    fn parse_or(&mut self) -> Result<bool> {
        let mut value = self.parse_and()?;
        while self.consume("|") {
            let right = self.parse_and()?;
            value |= right;
        }
        Ok(value)
    }

    fn parse_and(&mut self) -> Result<bool> {
        let mut value = self.parse_primary()?;
        while self.consume("&") {
            let right = self.parse_primary()?;
            value &= right;
        }
        Ok(value)
    }

    fn parse_primary(&mut self) -> Result<bool> {
        if self.consume("(") {
            let value = self.parse_or()?;
            if !self.consume(")") {
                bail!("Expected ')'");
            }
            return Ok(value);
        }
        let slot = self.parse_name()?;
        match slot {
            "on" => return Ok(true),
            "off" => return Ok(false),
            _ => {}
        }
        let Some((_, selected)) = self.edges.iter().find(|(name, _)| name == slot) else {
            bail!("Unknown edge slot: {slot}");
        };
        if !self.consume(":") {
            bail!("Expected ':' after {slot}");
        }
        let mut matches = false;
        loop {
            let variant = self.parse_name()?;
            if !self.variants.iter().any(|name| name == variant) {
                bail!("Unknown edge variant: {variant}");
            }
            matches |= selected == variant;
            if !self.consume(",") {
                break;
            }
        }
        Ok(matches)
    }
}
