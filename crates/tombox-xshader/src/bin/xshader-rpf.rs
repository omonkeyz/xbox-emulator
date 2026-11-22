use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tombox_xshader::{rpf, Stage};

const USAGE: &str =
    "usage: xshader-rpf <shaders.rpf> [--stage <stage>] [--out <directory>]\n\
     stages: pixel, vertex, geometry, hull, domain, compute, mesh, amplification, unknown";

struct Options {
    archive: PathBuf,
    stage: Option<String>,
    output: Option<PathBuf>,
}

fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<Option<Options>, String> {
    let archive = args.next().ok_or("missing archive path")?;
    if archive == "--help" || archive == "-h" {
        return Ok(None);
    }
    let mut options = Options {
        archive: archive.into(),
        stage: None,
        output: None,
    };
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--stage") if options.stage.is_none() => {
                let value = args.next().ok_or("missing stage")?.to_string_lossy().to_ascii_lowercase();
                if !matches!(
                    value.as_str(),
                    "pixel" | "vertex" | "geometry" | "hull" | "domain" | "compute"
                        | "mesh" | "amplification" | "unknown"
                ) {
                    return Err(format!("unknown stage {value}"));
                }
                options.stage = Some(value);
            }
            Some("--out") if options.output.is_none() => {
                options.output = Some(args.next().ok_or("missing output directory")?.into());
            }
            _ => return Err(format!("unexpected argument {}", flag.to_string_lossy())),
        }
    }
    Ok(Some(options))
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Pixel => "pixel",
        Stage::Vertex => "vertex",
        Stage::Geometry => "geometry",
        Stage::Hull => "hull",
        Stage::Domain => "domain",
        Stage::Compute => "compute",
        Stage::Mesh => "mesh",
        Stage::Amplification => "amplification",
        Stage::Unknown(_) => "unknown",
    }
}

fn write_shader(path: &Path, dxbc: &[u8]) -> io::Result<()> {
    let mut file = match File::create_new(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let mut existing = Vec::new();
            File::open(path)?.take(dxbc.len() as u64 + 1).read_to_end(&mut existing)?;
            if existing == dxbc {
                return Ok(());
            }
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    if let Err(error) = file.write_all(dxbc) {
        drop(file);
        fs::remove_file(path)?;
        return Err(error);
    }
    Ok(())
}

fn run(options: Options) -> io::Result<()> {
    let archive = File::open(&options.archive)?;
    let mut output = BufWriter::new(io::stdout().lock());
    writeln!(output, "offset\tname\tstage\tshader_model\tcompressed_bytes\tdxbc_bytes")?;
    let mut found = 0;
    let mut selected = 0;
    rpf::scan(archive, |record| {
        found += 1;
        let stage = stage_name(record.stage);
        if options.stage.as_deref().is_some_and(|filter| filter != stage) {
            return Ok(());
        }
        if let Some(directory) = &options.output {
            if selected == 0 {
                fs::create_dir_all(directory)?;
            }
            let path = directory.join(format!("{:012x}.dxbc", record.offset));
            write_shader(&path, record.dxbc)
                .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))?;
        }
        selected += 1;
        writeln!(
            output,
            "{:012x}\t{}\t{}\t{}.{}\t{}\t{}",
            record.offset, record.name, stage, record.shader_model.0, record.shader_model.1,
            record.compressed_size, record.dxbc.len(),
        )
    })?;
    output.flush()?;
    eprintln!("{found} shader records found, {selected} selected");
    Ok(())
}

fn main() -> ExitCode {
    match parse_args(std::env::args_os().skip(1)) {
        Ok(Some(options)) => match run(options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("xshader-rpf: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(None) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("xshader-rpf: {error}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
