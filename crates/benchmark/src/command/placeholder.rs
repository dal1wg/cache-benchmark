use crate::command::distribution::DistributionEnum;
use crate::command::{CommandGenerator, parser};
use rand::distributions::Alphanumeric;
use rand::prelude::*;
use std::cmp::min;
use std::process::exit;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum PlaceholderEnum {
    String(PlaceholderString),
    Key(PlaceholderKey),
    Value(PlaceholderValue),
    Rand(PlaceholderRand),
    Range(PlaceholderRange),
}

impl PlaceholderEnum {
    pub fn new_string(str: &str) -> Self {
        Self::String(PlaceholderString::new(str.to_string()))
    }
    /// CLI semantics: invalid placeholders terminate the process.
    pub fn new(str: &str) -> Self {
        match Self::try_new(str) {
            Ok(p) => p,
            Err(msg) => {
                eprintln!("{}", msg);
                exit(1);
            }
        }
    }
    /// Testable variant: returns an error message instead of exiting.
    pub fn try_new(str: &str) -> Result<Self, String> {
        let s = str.to_string();
        let words: Vec<&str> = s.split_whitespace().collect();
        if words.is_empty() {
            return Err("placeholder is empty".to_string());
        }
        let ph = match words[0] {
            "key" => {
                if words.len() != 3 {
                    return Err(format!("wrong number of arguments for key placeholder: {:?}", words));
                }
                let range = words[2].parse::<u64>().map_err(|e| format!("invalid key range {:?}: {}", words[2], e))?;
                let distribution = DistributionEnum::try_new(words[1], range)?;
                PlaceholderEnum::Key(PlaceholderKey::new(distribution))
            }
            "value" => {
                if words.len() != 2 {
                    return Err(format!("wrong number of arguments for value placeholder: {:?}", words));
                }
                let size = words[1].parse::<u64>().map_err(|e| format!("invalid value size {:?}: {}", words[1], e))?;
                PlaceholderEnum::Value(PlaceholderValue::new(size))
            }
            "rand" => {
                if words.len() != 2 {
                    return Err(format!("wrong number of arguments for rand placeholder: {:?}", words));
                }
                let range = words[1].parse::<u64>().map_err(|e| format!("invalid rand range {:?}: {}", words[1], e))?;
                PlaceholderEnum::Rand(PlaceholderRand::new(range))
            }
            "range" => {
                if words.len() != 3 {
                    return Err(format!("wrong number of arguments for range placeholder: {:?}", words));
                }
                let range = words[1].parse::<u64>().map_err(|e| format!("invalid range N {:?}: {}", words[1], e))?;
                let width = words[2].parse::<u64>().map_err(|e| format!("invalid width W {:?}: {}", words[2], e))?;
                PlaceholderEnum::Range(PlaceholderRange::new(range, width))
            }
            name => {
                return Err(format!("Invalid placeholder: {}", name));
            }
        };
        Ok(ph)
    }
    pub fn generate(&mut self) -> Vec<String> {
        match self {
            Self::String(p) => vec![p.generate()],
            Self::Key(p) => vec![p.generate()],
            Self::Value(p) => vec![p.generate()],
            Self::Rand(p) => vec![p.generate()],
            Self::Range(p) => p.generate(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlaceholderString {
    value: String,
}

impl PlaceholderString {
    pub fn new(value: String) -> Self {
        Self { value }
    }
    fn generate(&mut self) -> String {
        self.value.clone()
    }
}

#[derive(Clone, Debug)]
pub struct PlaceholderKey {
    distribution: DistributionEnum,
}

impl PlaceholderKey {
    fn new(distribution: DistributionEnum) -> Self {
        Self { distribution }
    }
    fn generate(&mut self) -> String {
        format!("key_{:010}", self.distribution.sample(&mut rand::thread_rng()))
    }
}

#[derive(Clone, Debug)]
pub struct PlaceholderValue {
    size: usize,
}

impl PlaceholderValue {
    pub fn new(size: u64) -> Self {
        Self { size: size as usize }
    }
    pub fn generate(&self) -> String {
        let rng = rand::thread_rng();
        let chars: String = rng.sample_iter(Alphanumeric).take(self.size).map(char::from).collect();
        chars
    }
}

#[derive(Clone, Debug)]
pub struct PlaceholderRand {
    distribution: DistributionEnum,
}

impl PlaceholderRand {
    pub fn new(range: u64) -> Self {
        Self { distribution: DistributionEnum::new("uniform", range) }
    }
    fn generate(&mut self) -> String {
        format!("{}", self.distribution.sample(&mut rand::thread_rng()))
    }
}

#[derive(Clone, Debug)]
pub struct PlaceholderRange {
    distribution: DistributionEnum,
    range: u64,
    width: u64,
}

impl PlaceholderRange {
    pub fn new(range: u64, width: u64) -> Self {
        Self {
            distribution: DistributionEnum::new("uniform", range),
            range,
            width,
        }
    }
    fn generate(&mut self) -> Vec<String> {
        let left = self.distribution.sample(&mut rand::thread_rng());
        let right = min(left + self.width, self.range - 1);
        vec![left.to_string(), right.to_string()]
    }
}

#[derive(Clone, Debug)]
pub struct PlaceholderGenerator {
    str: String,
    argv: Vec<PlaceholderEnum>,
    lock: Arc<Mutex<()>>,
}

impl PlaceholderGenerator {
    /// CLI semantics: panics on parse error.
    pub fn new(cmd: &str) -> PlaceholderGenerator {
        match Self::try_new(cmd) {
            Ok(g) => g,
            Err(e) => panic!("cmd parse error. cmd: {}, error: {}", cmd, e),
        }
    }
    /// Testable variant: returns a Result.
    pub fn try_new(cmd: &str) -> Result<PlaceholderGenerator, String> {
        match parser::parse_all(cmd) {
            Ok((nm, args)) => {
                if !nm.is_empty() {
                    return Err(format!("trailing input: {:?}", nm));
                }
                Ok(PlaceholderGenerator {
                    str: cmd.to_string(),
                    argv: args,
                    lock: Arc::new(Mutex::new(())),
                })
            }
            Err(e) => Err(format!("{:?}", e)),
        }
    }
}

impl CommandGenerator for PlaceholderGenerator {
    fn gen_cmd(&mut self) -> redis::Cmd {
        let mut cmd = redis::Cmd::new();
        let mut cmd_str = String::new();
        for ph in self.argv.iter_mut() {
            for arg in ph.generate() {
                if !cmd_str.is_empty() && !cmd_str.ends_with(char::is_whitespace) {
                    // separate multi-token placeholders ({range N W} yields two
                    // numbers) and placeholder/literal boundaries with a space
                    cmd_str.push(' ');
                }
                cmd_str.push_str(&arg);
            }
        }
        for word in cmd_str.split_whitespace() {
            cmd.arg(word);
        }
        cmd
    }

    fn gen_cmd_with_lock(&mut self) -> redis::Cmd {
        let _lock = self.lock.lock().unwrap();
        let mut cmd = redis::Cmd::new();
        let mut cmd_str = String::new();
        for ph in self.argv.iter_mut() {
            for arg in ph.generate() {
                if !cmd_str.is_empty() && !cmd_str.ends_with(char::is_whitespace) {
                    cmd_str.push(' ');
                }
                cmd_str.push_str(&arg);
            }
        }
        for word in cmd_str.split_whitespace() {
            cmd.arg(word);
        }
        cmd
    }

    fn clone_box(&self) -> Box<dyn CommandGenerator> {
        Box::new(self.clone())
    }
}

impl std::string::ToString for PlaceholderGenerator {
    fn to_string(&self) -> String {
        self.str.clone()
    }
}
