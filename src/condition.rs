use std::collections::BTreeMap;

use anyhow::{bail, Result};

pub fn evaluate_condition(
    condition: &str,
    edges: &[(String, String)],
    variants: &[String],
    aliases: &BTreeMap<String, Vec<String>>,
    slot_aliases: &BTreeMap<String, String>,
) -> Result<bool> {
    let mut parser = Parser {
        remaining: condition.trim(),
        edges,
        variants,
        aliases,
        slot_aliases,
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
    aliases: &'a BTreeMap<String, Vec<String>>,
    slot_aliases: &'a BTreeMap<String, String>,
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
        if self.consume("!") {
            return Ok(!self.parse_primary()?);
        }
        if self.consume("(") {
            let value = self.parse_or()?;
            if !self.consume(")") {
                bail!("Expected ')'");
            }
            return Ok(value);
        }
        let slot = self.parse_name()?;
        match slot {
            "true" => return Ok(true),
            "false" => return Ok(false),
            _ => {}
        }
        let slot = if let Some(target) = self.slot_aliases.get(slot) {
            if self.edges.iter().any(|(name, _)| name == slot) {
                bail!("Ambiguous slot name: {slot} is both a slot and an alias");
            }
            target.as_str()
        } else {
            slot
        };
        let Some((_, selected)) = self.edges.iter().find(|(name, _)| name == slot) else {
            bail!("Unknown edge slot: {slot}");
        };
        if !self.consume(":") {
            bail!("Expected ':' after {slot}");
        }
        let mut matches = false;
        loop {
            let name = self.parse_name()?;
            if let Some(variants) = self.aliases.get(name) {
                if self.variants.iter().any(|variant| variant == name) {
                    bail!("Ambiguous edge name: {name} is both a variant and an alias");
                }
                for variant in variants {
                    if !self.variants.contains(variant) {
                        bail!("Unknown edge variant {variant} in alias {name}");
                    }
                    matches |= selected == variant;
                }
            } else {
                if !self.variants.iter().any(|variant| variant == name) {
                    bail!("Unknown edge variant or alias: {name}");
                }
                matches |= selected == name;
            }
            if !self.consume(",") {
                break;
            }
        }
        Ok(matches)
    }
}
