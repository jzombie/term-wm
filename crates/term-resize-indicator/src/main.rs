use clap::Parser;
use term_resize_indicator::{run, Size};

#[derive(Parser)]
#[command(version, about = "Dotted-line border overlay showing terminal dimensions on resize, with lock-to-size.")]
struct Args {
    #[arg(
        long,
        value_name = "WxH",
        help = "Set terminal to given size and lock (e.g. 80x24)"
    )]
    size: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let initial_lock = args
        .size
        .as_deref()
        .map(parse_size)
        .transpose()?;

    run(initial_lock)?;
    Ok(())
}

fn parse_size(s: &str) -> Result<Size, String> {
    let parts: Vec<&str> = s.split('x').collect();
    if parts.len() != 2 {
        return Err(format!(
            "Invalid size format '{}': expected WxH (e.g. 80x24)",
            s
        ));
    }
    let width = parts[0]
        .parse::<u16>()
        .map_err(|_| format!("Invalid width '{}'", parts[0]))?;
    let height = parts[1]
        .parse::<u16>()
        .map_err(|_| format!("Invalid height '{}'", parts[1]))?;
    if width == 0 || height == 0 {
        return Err("Width and height must be non-zero".into());
    }
    Ok(Size::new(width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_valid() {
        assert_eq!(parse_size("80x24"), Ok(Size::new(80, 24)));
        assert_eq!(parse_size("120x40"), Ok(Size::new(120, 40)));
    }

    #[test]
    fn parse_size_invalid() {
        assert!(parse_size("80").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("80x").is_err());
        assert!(parse_size("x24").is_err());
        assert!(parse_size("0x24").is_err());
        assert!(parse_size("80x0").is_err());
    }
}
