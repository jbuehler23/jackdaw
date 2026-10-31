//! Region text: a plain-text rectangle of cells, one character per cell.
//!
//! ```text
//! legend: { ".": "empty", "#": "wall", "g": "grass", "C": "tile:chest" }
//! rows:
//!   ##########
//!   #gggg..gg#
//! ```
//!
//! Rows run top to bottom and the origin names the top-left cell. The world
//! is y-up, so row `r`, column `c` is cell `(origin.x + c, origin.y - r)`.
//! Legend values are `"empty"` (or `"erase"`), `"keep"` to leave a cell as
//! it is, `"tile:NAME"` for an exact tile, `"terrain:NAME"` for a terrain
//! whatever its name, or any other name for a terrain.
//! Resolving names to ids is the caller's job; this module
//! only parses, validates and emits the text. The surrounding `legend:` and
//! `rows:` framing is also the caller's; [`parse_rows`] takes the row block.

use std::collections::BTreeMap;
use std::fmt;

use glam::IVec2;

use crate::grid::{CellCoord, CellRect};
use crate::layer::{CellValue, ChunkedLayer};

use RegionTextErrorKind as ErrorKind;

const TILE_PREFIX: &str = "tile:";
const TERRAIN_PREFIX: &str = "terrain:";

/// Values that never name a terrain unless written after `terrain:`.
const KEYWORDS: [&str; 3] = ["empty", "erase", "keep"];

/// What a legend symbol stands for.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LegendEntry {
    Empty,
    /// Leaves the cell as it is when written.
    Keep,
    Terrain(String),
    Tile(String),
}

impl LegendEntry {
    /// Parses a legend value. Returns `None` for an empty name (`""`,
    /// `"tile:"` or `"terrain:"`).
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "empty" | "erase" => return Some(Self::Empty),
            "keep" => return Some(Self::Keep),
            _ => {}
        }
        if let Some(name) = value.strip_prefix(TERRAIN_PREFIX) {
            return (!name.is_empty()).then(|| Self::Terrain(name.to_string()));
        }
        if let Some(name) = value.strip_prefix(TILE_PREFIX) {
            if name.is_empty() {
                return None;
            }
            return Some(Self::Tile(name.to_string()));
        }
        if value.is_empty() {
            return None;
        }
        Some(Self::Terrain(value.to_string()))
    }
}

/// Writes the legend value form, the inverse of [`LegendEntry::parse`].
impl fmt::Display for LegendEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("empty"),
            Self::Keep => f.write_str("keep"),
            Self::Terrain(name) if needs_prefix(name) => write!(f, "{TERRAIN_PREFIX}{name}"),
            Self::Terrain(name) => f.write_str(name),
            Self::Tile(name) => write!(f, "{TILE_PREFIX}{name}"),
        }
    }
}

/// Whether a terrain name would parse as something else written bare.
fn needs_prefix(name: &str) -> bool {
    KEYWORDS.contains(&name) || name.starts_with(TILE_PREFIX) || name.starts_with(TERRAIN_PREFIX)
}

/// Symbol table for region rows, ordered by symbol.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Legend {
    entries: BTreeMap<char, LegendEntry>,
}

impl Legend {
    /// Builds a legend from `(key, value)` pairs. Keys must be exactly one
    /// printable ASCII character other than space; each key may appear once.
    /// Errors are unpositioned (line and column 0).
    pub fn parse_entries<'a>(
        pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, RegionTextError> {
        let mut legend = Self::default();
        for (key, value) in pairs {
            let symbol = legend_key(key)?;
            let entry = LegendEntry::parse(value)
                .ok_or_else(|| RegionTextError::unpositioned(ErrorKind::EmptyName(symbol)))?;
            legend.insert(symbol, entry)?;
        }
        Ok(legend)
    }

    /// Adds one symbol, rejecting invalid and duplicate keys and entries that
    /// would not survive writing and parsing again.
    pub fn insert(&mut self, symbol: char, entry: LegendEntry) -> Result<(), RegionTextError> {
        validate_symbol(symbol).map_err(RegionTextError::unpositioned)?;
        validate_entry(symbol, &entry).map_err(RegionTextError::unpositioned)?;
        if self.entries.contains_key(&symbol) {
            return Err(RegionTextError::unpositioned(ErrorKind::DuplicateKey(
                symbol,
            )));
        }
        self.entries.insert(symbol, entry);
        Ok(())
    }

    pub fn get(&self, symbol: char) -> Option<&LegendEntry> {
        self.entries.get(&symbol)
    }

    /// The lowest symbol mapped to `entry`, for emitting.
    pub fn symbol_for(&self, entry: &LegendEntry) -> Result<char, RegionTextError> {
        self.entries
            .iter()
            .find(|(_, candidate)| *candidate == entry)
            .map(|(symbol, _)| *symbol)
            .ok_or_else(|| RegionTextError::unpositioned(ErrorKind::NoSymbol(entry.clone())))
    }

    pub fn iter(&self) -> impl Iterator<Item = (char, &LegendEntry)> {
        self.entries.iter().map(|(symbol, entry)| (*symbol, entry))
    }
}

fn validate_symbol(symbol: char) -> Result<(), ErrorKind> {
    if symbol.is_ascii_graphic() {
        Ok(())
    } else {
        Err(ErrorKind::InvalidKey(symbol.to_string()))
    }
}

fn validate_entry(symbol: char, entry: &LegendEntry) -> Result<(), ErrorKind> {
    match entry {
        LegendEntry::Terrain(name) | LegendEntry::Tile(name) if name.is_empty() => {
            Err(ErrorKind::EmptyName(symbol))
        }
        _ if LegendEntry::parse(&entry.to_string()).as_ref() != Some(entry) => {
            Err(ErrorKind::ReservedName(entry.to_string()))
        }
        LegendEntry::Empty | LegendEntry::Keep | LegendEntry::Terrain(_) | LegendEntry::Tile(_) => {
            Ok(())
        }
    }
}

fn legend_key(key: &str) -> Result<char, RegionTextError> {
    let mut chars = key.chars();
    if let Some(symbol) = chars.next()
        && chars.next().is_none()
    {
        return validate_symbol(symbol)
            .map(|()| symbol)
            .map_err(RegionTextError::unpositioned);
    }
    Err(RegionTextError::unpositioned(ErrorKind::InvalidKey(
        key.to_string(),
    )))
}

/// Whether a region of this size at `origin` has every cell and its
/// half-open bounds inside i32 range. Zero sizes never fit.
fn extent_fits(origin: CellCoord, width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    let max_x = i64::from(origin.0.x) + i64::from(width);
    let min_y = i64::from(origin.0.y) - (i64::from(height) - 1);
    let max_y = i64::from(origin.0.y) + 1;
    max_x <= i64::from(i32::MAX) && max_y <= i64::from(i32::MAX) && min_y >= i64::from(i32::MIN)
}

/// A non-empty rectangle of symbols, stored row-major with the top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    /// The top-left cell.
    pub origin: CellCoord,
    pub width: u32,
    pub height: u32,
    cells: Vec<char>,
}

impl Region {
    /// Builds a region from row-major symbols, top row first. Returns `None`
    /// when `cells.len()` is not `width * height`, when either dimension is
    /// zero, or when the region or its half-open bounds leave i32 range.
    pub fn from_cells(
        origin: CellCoord,
        width: u32,
        height: u32,
        cells: Vec<char>,
    ) -> Option<Self> {
        let expected = width as usize * height as usize;
        if cells.len() != expected || !extent_fits(origin, width, height) {
            return None;
        }
        Some(Self {
            origin,
            width,
            height,
            cells,
        })
    }

    pub fn cell(&self, col: u32, row: u32) -> Option<char> {
        if col >= self.width || row >= self.height {
            return None;
        }
        Some(self.cells[row as usize * self.width as usize + col as usize])
    }

    /// World cell of a column and row.
    pub fn world(&self, col: u32, row: u32) -> CellCoord {
        CellCoord::new(self.origin.0.x + col as i32, self.origin.0.y - row as i32)
    }

    /// Every cell in row-major text order, with its world coordinate.
    pub fn cells(&self) -> impl Iterator<Item = (CellCoord, char)> + '_ {
        self.cells.iter().enumerate().map(|(index, symbol)| {
            let width = self.width as usize;
            let (col, row) = (index % width, index / width);
            (self.world(col as u32, row as u32), *symbol)
        })
    }

    /// The covered cells as a half-open y-up rectangle.
    pub fn bounds(&self) -> CellRect {
        let min = IVec2::new(self.origin.0.x, self.origin.0.y + 1 - self.height as i32);
        CellRect {
            min,
            max: IVec2::new(min.x + self.width as i32, self.origin.0.y + 1),
        }
    }
}

/// A region text error. Row errors carry the 1-based line and column of the
/// offending character in the input string as given, counted in characters,
/// before blank lines are skipped or indentation removed. Legend errors are
/// unpositioned and use line 0, column 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionTextError {
    pub line: usize,
    pub column: usize,
    pub kind: RegionTextErrorKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegionTextErrorKind {
    UnknownSymbol(char),
    /// A row whose length differs from the first row's.
    RaggedRow {
        expected: usize,
        found: usize,
    },
    /// A region whose cells or bounds would leave i32 range.
    OutOfRange,
    /// A blank line between the first and last row.
    BlankRow,
    /// No rows at all.
    Empty,
    InvalidKey(String),
    DuplicateKey(char),
    /// A legend value of `""` or `"tile:"`.
    EmptyName(char),
    /// An entry that would not parse back as itself.
    ReservedName(String),
    /// An entry has no symbol in the legend.
    NoSymbol(LegendEntry),
}

impl RegionTextError {
    fn at(line: usize, column: usize, kind: ErrorKind) -> Self {
        Self { line, column, kind }
    }

    fn unpositioned(kind: ErrorKind) -> Self {
        Self::at(0, 0, kind)
    }
}

impl fmt::Display for RegionTextErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSymbol(symbol) => write!(f, "unknown symbol {symbol:?}"),
            Self::RaggedRow { expected, found } => {
                write!(f, "row has {found} cells, expected {expected}")
            }
            Self::BlankRow => f.write_str("blank row inside the region"),
            Self::Empty => f.write_str("region has no rows"),
            Self::InvalidKey(key) => write!(
                f,
                "invalid legend key {key:?}: expected one printable ASCII character other than space"
            ),
            Self::DuplicateKey(symbol) => write!(f, "duplicate legend key {symbol:?}"),
            Self::EmptyName(symbol) => write!(f, "legend entry {symbol:?} has an empty name"),
            Self::OutOfRange => f.write_str("region extends past the coordinate range"),
            Self::ReservedName(name) => write!(f, "reserved terrain name {name:?}"),
            Self::NoSymbol(entry) => write!(f, "no legend symbol for {:?}", entry.to_string()),
        }
    }
}

impl fmt::Display for RegionTextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            return write!(f, "{}", self.kind);
        }
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.kind
        )
    }
}

impl std::error::Error for RegionTextError {}

fn is_blank(line: &str) -> bool {
    line.chars().all(|c| c == ' ' || c == '\t')
}

fn indent_of(line: &str) -> &str {
    let rest = line.trim_start_matches([' ', '\t']);
    &line[..line.len() - rest.len()]
}

fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let len = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    &a[..len]
}

/// Parses a block of rows whose top-left cell is `origin`. Leading and
/// trailing blank lines are skipped and indentation shared by every row is
/// removed; every other character must be a legend symbol.
pub fn parse_rows(
    rows: &str,
    origin: CellCoord,
    legend: &Legend,
) -> Result<Region, RegionTextError> {
    let lines: Vec<&str> = rows
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let Some(first) = lines.iter().position(|line| !is_blank(line)) else {
        return Err(RegionTextError::at(1, 1, ErrorKind::Empty));
    };
    let last = lines
        .iter()
        .rposition(|line| !is_blank(line))
        .unwrap_or(first);
    let body = &lines[first..=last];

    let mut indent = indent_of(body[0]);
    for (offset, line) in body.iter().enumerate() {
        if is_blank(line) {
            return Err(RegionTextError::at(
                first + offset + 1,
                1,
                ErrorKind::BlankRow,
            ));
        }
        indent = common_prefix(indent, indent_of(line));
    }
    let indent_columns = indent.chars().count();

    let width = body[0][indent.len()..].chars().count();
    let mut cells = Vec::with_capacity(width * body.len());
    for (offset, line) in body.iter().enumerate() {
        let line_no = first + offset + 1;
        let mut found = 0;
        for (index, symbol) in line[indent.len()..].chars().enumerate() {
            let column = indent_columns + index + 1;
            if index >= width {
                let found = line[indent.len()..].chars().count();
                return Err(RegionTextError::at(
                    line_no,
                    column,
                    ErrorKind::RaggedRow {
                        expected: width,
                        found,
                    },
                ));
            }
            if legend.get(symbol).is_none() {
                return Err(RegionTextError::at(
                    line_no,
                    column,
                    ErrorKind::UnknownSymbol(symbol),
                ));
            }
            cells.push(symbol);
            found += 1;
        }
        if found < width {
            return Err(RegionTextError::at(
                line_no,
                indent_columns + found + 1,
                ErrorKind::RaggedRow {
                    expected: width,
                    found,
                },
            ));
        }
    }
    let size = u32::try_from(width)
        .ok()
        .zip(u32::try_from(body.len()).ok());
    match size {
        Some((width, height)) if extent_fits(origin, width, height) => Ok(Region {
            origin,
            width,
            height,
            cells,
        }),
        _ => Err(RegionTextError::at(first + 1, 1, ErrorKind::OutOfRange)),
    }
}

/// Writes the rows top to bottom, one line each, with a trailing newline.
pub fn emit_rows(region: &Region) -> String {
    let mut out = String::with_capacity(region.cells.len() + region.height as usize);
    for row in region.cells.chunks(region.width as usize) {
        out.extend(row);
        out.push('\n');
    }
    out
}

/// Why [`read_region`] failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadRegionError<T> {
    /// The rectangle covers no cells.
    EmptyRect,
    /// `symbol` has no character for the value at `cell`.
    NoSymbol { cell: CellCoord, value: T },
}

impl<T: fmt::Debug> fmt::Display for ReadRegionError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRect => f.write_str("rectangle covers no cells"),
            Self::NoSymbol { cell, value } => {
                write!(f, "no symbol for {value:?} at ({}, {})", cell.0.x, cell.0.y)
            }
        }
    }
}

impl<T: fmt::Debug> std::error::Error for ReadRegionError<T> {}

/// Reads the cells of `rect` into a region whose origin is its top-left cell
/// and whose bounds equal `rect`. Fails on an empty rectangle or on the first
/// value `symbol` has no character for.
pub fn read_region<T: CellValue>(
    layer: &ChunkedLayer<T>,
    rect: CellRect,
    symbol: impl Fn(T) -> Option<char>,
) -> Result<Region, ReadRegionError<T>> {
    let size = rect.size();
    if size.x == 0 || size.y == 0 {
        return Err(ReadRegionError::EmptyRect);
    }
    let origin = CellCoord::new(rect.min.x, rect.max.y - 1);
    let mut region = Region {
        origin,
        width: size.x,
        height: size.y,
        cells: Vec::with_capacity(size.x as usize * size.y as usize),
    };
    for row in 0..size.y {
        for col in 0..size.x {
            let cell = region.world(col, row);
            let value = layer.get(cell);
            let Some(ch) = symbol(value) else {
                return Err(ReadRegionError::NoSymbol { cell, value });
            };
            region.cells.push(ch);
        }
    }
    Ok(region)
}

/// Writes every cell of `region` into `layer`.
pub fn write_region<T: CellValue>(
    layer: &mut ChunkedLayer<T>,
    region: &Region,
    value: impl Fn(char) -> T,
) {
    for (cell, symbol) in region.cells() {
        layer.set(cell, value(symbol));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legend() -> Legend {
        Legend::parse_entries([
            (".", "empty"),
            ("#", "wall"),
            ("g", "grass"),
            ("C", "tile:chest"),
        ])
        .expect("valid legend")
    }

    fn parse(rows: &str) -> Result<Region, RegionTextError> {
        parse_rows(rows, CellCoord::new(0, 0), &legend())
    }

    fn err(rows: &str) -> (usize, usize, ErrorKind) {
        let error = parse(rows).expect_err("should fail");
        (error.line, error.column, error.kind)
    }

    fn legend_err(pairs: &[(&str, &str)]) -> ErrorKind {
        let error = Legend::parse_entries(pairs.iter().copied()).expect_err("should fail");
        assert_eq!((error.line, error.column), (0, 0));
        error.kind
    }

    #[test]
    fn rows_map_to_y_up_cells() {
        let region = parse_rows("#g\n.C\n", CellCoord::new(-3, -5), &legend()).unwrap();
        let cells: Vec<_> = region.cells().collect();
        assert_eq!(
            cells,
            vec![
                (CellCoord::new(-3, -5), '#'),
                (CellCoord::new(-2, -5), 'g'),
                (CellCoord::new(-3, -6), '.'),
                (CellCoord::new(-2, -6), 'C'),
            ]
        );
        assert_eq!(region.cell(1, 1), Some('C'));
        assert_eq!(region.cell(2, 0), None);
        assert_eq!(
            region.bounds(),
            CellRect {
                min: IVec2::new(-3, -6),
                max: IVec2::new(-1, -4),
            }
        );
    }

    #[test]
    fn strips_shared_indentation_and_blank_edges() {
        let region = parse("\n\n    ##\n    g.\n  \n").unwrap();
        assert_eq!((region.width, region.height), (2, 2));
        assert_eq!(emit_rows(&region), "##\ng.\n");
    }

    #[test]
    fn accepts_crlf() {
        let region = parse("##\r\ng.\r\n").unwrap();
        assert_eq!(emit_rows(&region), "##\ng.\n");
    }

    #[test]
    fn unknown_symbol_position_accounts_for_skipped_lines_and_indent() {
        assert_eq!(
            err("\n\n  ##\n  #x\n"),
            (4, 4, ErrorKind::UnknownSymbol('x'))
        );
        assert_eq!(err("## \n##"), (1, 3, ErrorKind::UnknownSymbol(' ')));
    }

    #[test]
    fn ragged_rows_point_past_the_shorter_end() {
        let short = ErrorKind::RaggedRow {
            expected: 3,
            found: 2,
        };
        assert_eq!(err("\n  ###\n  ##\n"), (3, 5, short));
        let long = ErrorKind::RaggedRow {
            expected: 2,
            found: 4,
        };
        assert_eq!(err("##\n####\n"), (2, 3, long));
    }

    #[test]
    fn interior_blank_line_is_an_error() {
        assert_eq!(err("\n##\n  \n##\n"), (3, 1, ErrorKind::BlankRow));
    }

    #[test]
    fn no_rows_is_an_error() {
        assert_eq!(err(""), (1, 1, ErrorKind::Empty));
        assert_eq!(err("  \n\t\n"), (1, 1, ErrorKind::Empty));
    }

    #[test]
    fn error_display_names_position() {
        let error = parse("##\n#x\n").unwrap_err();
        assert_eq!(error.to_string(), "line 2, column 2: unknown symbol 'x'");
    }

    #[test]
    fn legend_rejects_bad_keys_and_names() {
        assert_eq!(
            legend_err(&[(" ", "empty")]),
            ErrorKind::InvalidKey(" ".into())
        );
        assert_eq!(
            legend_err(&[("\t", "empty")]),
            ErrorKind::InvalidKey("\t".into())
        );
        assert_eq!(
            legend_err(&[("ab", "wall")]),
            ErrorKind::InvalidKey("ab".into())
        );
        assert_eq!(
            legend_err(&[("", "wall")]),
            ErrorKind::InvalidKey("".into())
        );
        assert_eq!(
            legend_err(&[("#", "wall"), ("#", "rock")]),
            ErrorKind::DuplicateKey('#')
        );
        assert_eq!(legend_err(&[("C", "tile:")]), ErrorKind::EmptyName('C'));
        assert_eq!(legend_err(&[("C", "")]), ErrorKind::EmptyName('C'));
    }

    #[test]
    fn keywords_are_never_bare_terrain_names() {
        assert_eq!(LegendEntry::parse("keep"), Some(LegendEntry::Keep));
        assert_eq!(LegendEntry::parse("erase"), Some(LegendEntry::Empty));
        assert_eq!(LegendEntry::Keep.to_string(), "keep");
        assert_eq!(LegendEntry::parse("terrain:"), None);
        assert_eq!(LegendEntry::Terrain("grass".into()).to_string(), "grass");
        let mut legend = Legend::default();
        for (symbol, name) in [
            ('a', "empty"),
            ('b', "erase"),
            ('c', "keep"),
            ('d', "tile:a"),
            ('e', "terrain:b"),
        ] {
            let entry = LegendEntry::Terrain(name.into());
            assert_eq!(entry.to_string(), format!("terrain:{name}"));
            assert_eq!(LegendEntry::parse(&entry.to_string()), Some(entry.clone()));
            assert!(legend.insert(symbol, entry).is_ok(), "{name}");
        }
    }

    #[test]
    fn legend_entry_round_trips() {
        for value in ["empty", "grass", "tile:chest", "tile:a:b"] {
            let entry = LegendEntry::parse(value).unwrap();
            assert_eq!(entry.to_string(), value);
        }
        assert_eq!(LegendEntry::parse("empty"), Some(LegendEntry::Empty));
        assert_eq!(
            LegendEntry::parse("tile:chest"),
            Some(LegendEntry::Tile("chest".into()))
        );
        assert_eq!(
            LegendEntry::parse("grass"),
            Some(LegendEntry::Terrain("grass".into()))
        );
    }

    #[test]
    fn symbol_lookup_by_entry() {
        let legend = legend();
        assert_eq!(legend.symbol_for(&LegendEntry::Empty), Ok('.'));
        let missing = LegendEntry::Terrain("sand".into());
        assert_eq!(
            legend.symbol_for(&missing).unwrap_err().kind,
            ErrorKind::NoSymbol(missing)
        );
    }

    #[test]
    fn read_then_write_round_trips_a_layer() {
        let mut layer = ChunkedLayer::<u8>::new();
        layer.set(CellCoord::new(-1, 2), 1);
        layer.set(CellCoord::new(31, 0), 2);
        layer.set(CellCoord::new(32, -1), 1);
        let rect = CellRect {
            min: IVec2::new(-1, -1),
            max: IVec2::new(33, 3),
        };
        let symbol = |v: u8| b".#g".get(v as usize).map(|b| *b as char);
        let region = read_region(&layer, rect, symbol).unwrap();
        assert_eq!(region.origin, CellCoord::new(-1, 2));
        assert_eq!(region.bounds(), rect);
        assert_eq!(region.cell(0, 0), Some('#'));
        assert_eq!(region.cell(33, 3), Some('#'));

        let mut copy = ChunkedLayer::<u8>::new();
        write_region(&mut copy, &region, |c| {
            b".#g".iter().position(|b| *b as char == c).unwrap() as u8
        });
        assert_eq!(copy, layer);

        layer.set(CellCoord::new(0, 0), 9);
        assert_eq!(
            read_region(&layer, rect, symbol),
            Err(ReadRegionError::NoSymbol {
                cell: CellCoord::new(0, 0),
                value: 9
            })
        );
    }

    #[test]
    fn insert_validates_symbol_and_entry() {
        let invalid = |symbol, entry| {
            Legend::default()
                .insert(symbol, entry)
                .expect_err("should fail")
                .kind
        };
        assert_eq!(
            invalid('\t', LegendEntry::Empty),
            ErrorKind::InvalidKey("\t".into())
        );
        assert_eq!(
            invalid('x', LegendEntry::Terrain(String::new())),
            ErrorKind::EmptyName('x')
        );
        assert_eq!(
            invalid('x', LegendEntry::Tile(String::new())),
            ErrorKind::EmptyName('x')
        );
        let mut legend = Legend::default();
        for (symbol, entry) in [
            ('a', LegendEntry::Empty),
            ('b', LegendEntry::Terrain("grass".into())),
            ('c', LegendEntry::Tile("chest".into())),
        ] {
            legend.insert(symbol, entry).unwrap();
        }
        for (_, entry) in legend.iter() {
            assert_eq!(LegendEntry::parse(&entry.to_string()).as_ref(), Some(entry));
        }
    }

    #[test]
    fn no_symbol_display_quotes_the_entry() {
        let kind = ErrorKind::NoSymbol(LegendEntry::Terrain("a\"b".into()));
        assert_eq!(kind.to_string(), "no legend symbol for \"a\\\"b\"");
    }

    #[test]
    fn regions_are_never_empty() {
        let origin = CellCoord::new(0, 0);
        assert!(Region::from_cells(origin, 0, 0, vec![]).is_none());
        assert!(Region::from_cells(origin, 0, 2, vec![]).is_none());
        assert!(Region::from_cells(origin, 2, 0, vec![]).is_none());
    }

    #[test]
    fn from_cells_rejects_extents_past_i32() {
        let cells = || vec!['#'; 4];
        let x = CellCoord::new(i32::MAX - 1, 0);
        assert!(Region::from_cells(x, 2, 2, cells()).is_none());
        let y = CellCoord::new(0, i32::MIN);
        assert!(Region::from_cells(y, 2, 2, cells()).is_none());
        assert!(Region::from_cells(CellCoord::new(0, i32::MAX), 2, 2, cells()).is_none());
        let last_x = CellCoord::new(i32::MAX - 2, 0);
        assert!(Region::from_cells(last_x, 2, 2, cells()).is_some());
        let last_y = CellCoord::new(0, i32::MIN + 1);
        assert!(Region::from_cells(last_y, 2, 2, cells()).is_some());
        let region = Region::from_cells(
            CellCoord::new(i32::MAX - 2, i32::MAX - 1),
            2,
            1,
            vec!['#'; 2],
        )
        .unwrap();
        assert_eq!(region.bounds().max, IVec2::new(i32::MAX, i32::MAX));
    }

    #[test]
    fn parse_rows_rejects_extents_past_i32() {
        let error = parse_rows("##\n##\n", CellCoord::new(i32::MAX - 1, 0), &legend())
            .expect_err("should fail");
        assert_eq!((error.line, error.column), (1, 1));
        assert_eq!(error.kind, ErrorKind::OutOfRange);
        let error = parse_rows("\n##\n##\n", CellCoord::new(0, i32::MIN), &legend())
            .expect_err("should fail");
        assert_eq!((error.line, error.column), (2, 1));
        assert_eq!(error.kind, ErrorKind::OutOfRange);
    }

    #[test]
    fn read_region_rejects_empty_and_inverted_rects() {
        let layer = ChunkedLayer::<u8>::new();
        let symbol = |_: u8| Some('.');
        let rect = |min: (i32, i32), max: (i32, i32)| CellRect {
            min: IVec2::new(min.0, min.1),
            max: IVec2::new(max.0, max.1),
        };
        for empty in [
            rect((0, 0), (0, 3)),
            rect((0, 0), (3, 0)),
            rect((2, 2), (0, 0)),
        ] {
            assert_eq!(
                read_region(&layer, empty, symbol),
                Err(ReadRegionError::EmptyRect)
            );
        }
        assert_eq!(
            ReadRegionError::<u8>::EmptyRect.to_string(),
            "rectangle covers no cells"
        );
        assert_eq!(
            ReadRegionError::NoSymbol {
                cell: CellCoord::new(1, -2),
                value: 9u8
            }
            .to_string(),
            "no symbol for 9 at (1, -2)"
        );
    }

    #[test]
    fn wide_index_does_not_wrap() {
        let region = parse("#g\n.C\n").unwrap();
        assert_eq!(region.cell(1, 1), Some('C'));
        assert_eq!(region.cells().last(), Some((CellCoord::new(1, -1), 'C')));
    }

    #[test]
    fn from_cells_checks_length() {
        let origin = CellCoord::new(0, 0);
        assert!(Region::from_cells(origin, 2, 2, vec!['#'; 3]).is_none());
        assert!(Region::from_cells(origin, 2, 2, vec!['#'; 4]).is_some());
    }
}
