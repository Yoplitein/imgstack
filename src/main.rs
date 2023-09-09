#![allow(non_snake_case)]

use std::fs::OpenOptions;
use std::io::BufReader;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result as AResult};
use clap::{Parser, ValueEnum};
use image::buffer::ConvertBuffer;
use image::io::Reader as ImageReader;
use image::{image_dimensions, GenericImageView, Pixel, Rgb32FImage, RgbImage};

/// A tool to merge together batches of images, e.g. light painting or faking
/// long exposures.
#[derive(Debug, Parser)]
struct Args {
	/// Output image.
	#[arg(short, long)]
	output: PathBuf,

	/// Input images.
	#[arg(required = true)]
	inputs: Vec<PathBuf>,

	/// Image processing mode.
	#[arg(short, long, default_value = "sum")]
	mode: Mode,

	/// Allow overwriting output file.
	#[arg(short = 'y', long, default_value_t = false)]
	overwrite: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
	/// Saturating sum.
	Sum,

	/// Overflowing sum.
	SumOverflow,

	/// Per-channel minimum.
	Min,

	/// Per-channel maximum.
	Max,

	/// Per-channel average.
	#[value(alias = "avg")]
	Average,
}

fn main() -> AResult<()> {
	let args = Args::parse();
	#[cfg(debug_assertions)]
	dbg!(&args);

	let outFile = args.output;
	if outFile.is_dir() {
		return Err(anyhow!("Output file {outFile:?} is a directory"));
	}
	if outFile.exists() && !args.overwrite {
		return Err(anyhow!(
			"Output file {outFile:?} exists, refusing to overwrite"
		));
	}

	let (width, height) = image_dimensions(args.inputs.first().unwrap())
		.context("Querying initial image dimensions")?;
	for file in args.inputs.iter().skip(1) {
		if !file.exists() || !file.is_file() {
			return Err(anyhow!("Input file {file:?} does not exist"));
		}

		let (otherWidth, otherHeight) =
			image_dimensions(file).with_context(|| format!("Querying dimensions of {file:?}"))?;
		if width != otherWidth || height != otherHeight {
			return Err(anyhow!(
				"Input image {file:?} has mismatched dimensions: expected {}x{} but got {}x{}",
				width,
				height,
				otherWidth,
				otherHeight
			));
		}
	}

	let inputs = args.inputs.into_iter().map(|path| {
		// for use in lazy error messages
		let pathStr = format!("{path:?}");
		let pathStr = &*Box::leak(pathStr.into_boxed_str());

		eprintln!("Stacking {pathStr}");
		let file = OpenOptions::new()
			.read(true)
			.open(path)
			.with_context(|| format!("Opening {pathStr}"))?;
		let file = BufReader::new(file);

		let img = ImageReader::new(file)
			.with_guessed_format()
			.with_context(|| format!("Guessing format of {pathStr}"))?
			.decode()
			.with_context(|| format!("Decoding {pathStr}"))?;
		match &img {
			image::DynamicImage::ImageRgb8(_) => {},
			image::DynamicImage::ImageRgba8(_) => {
				eprintln!("Warning: alpha channel in {pathStr} will be discarded")
			},
			_ => return Err(anyhow!("Image {pathStr} has an unsupported pixel format")),
		}
		Ok(img)
	});

	let outImg = match args.mode {
		Mode::Sum | Mode::SumOverflow | Mode::Min | Mode::Max => {
			let mut outImg = RgbImage::new(width, height);
			let op = match args.mode {
				Mode::Sum => |acc: u8, samp: u8| acc.saturating_add(samp),
				Mode::SumOverflow => |acc: u8, samp: u8| acc.overflowing_add(samp).0,
				Mode::Min => |acc: u8, samp: u8| acc.min(samp),
				Mode::Max => |acc: u8, samp: u8| acc.max(samp),
				Mode::Average => unreachable!(),
			};
			for img in inputs {
				let img = img?;
				for (acc, (_, _, sample)) in outImg.pixels_mut().zip(img.pixels()) {
					let sample = sample.to_rgb();
					acc.apply2(&sample, op);
				}
			}
			outImg
		},
		Mode::Average => {
			let mut outImg = Rgb32FImage::new(width, height);
			let divisor = inputs.len() as f32;
			for img in inputs {
				let img = img?;
				for (acc, (_, _, sample)) in outImg.pixels_mut().zip(img.pixels()) {
					let sample = sample.to_rgb();
					let sample = sample.0.map(|v| v as f32 / 255.0).into();
					acc.apply2(&sample, |acc, sample| acc + sample);
				}
			}
			outImg.pixels_mut().for_each(|p| p.apply(|v| v / divisor));
			outImg.convert()
		},
	};
	outImg.save(outFile).context("Saving output file")?;

	Ok(())
}
