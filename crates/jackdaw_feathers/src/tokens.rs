use bevy::color::palettes::tailwind;
use bevy::prelude::*;

// ---------------------------------------------------------------------------
// Corner radii
// ---------------------------------------------------------------------------

pub const CORNER_RADIUS: Val = Val::Px(2.0);
pub const CORNER_RADIUS_LG: Val = Val::Px(4.0);

// ---------------------------------------------------------------------------
// Accent / primary
// ---------------------------------------------------------------------------

pub const PRIMARY_COLOR: Srgba = tailwind::BLUE_500;
/// Accent blue used for selections, active tabs, and highlights (#206EC8).
pub const ACCENT_BLUE: Color = Color::srgb(0.126, 0.431, 0.784);

// ---------------------------------------------------------------------------
// Backgrounds (from Figma CSS, updated palette, slightly bluer tones)
// ---------------------------------------------------------------------------

/// Root window / frame background (#1F1F24)
pub const WINDOW_BG: Color = Color::srgb(0.122, 0.122, 0.141);
/// Panel body / content background (#2A2A2E)
pub const PANEL_BG: Color = Color::srgb(0.165, 0.165, 0.180);
/// Panel header / tab bar background (#1F1F24)
pub const PANEL_HEADER_BG: Color = Color::srgb(0.122, 0.122, 0.141);
/// Toolbar background (#2A2A2E)
pub const TOOLBAR_BG: Color = Color::srgb(0.165, 0.165, 0.180);
/// Text input background (#36373B)
pub const INPUT_BG: Color = Color::srgb(0.212, 0.216, 0.231);
/// Context menu / dropdown background (#2A2A2E with near-opaque alpha)
pub const MENU_BG: Color = Color::srgba(0.165, 0.165, 0.180, 0.98);
/// Status bar background (#2A2A2E)
pub const STATUS_BAR_BG: Color = Color::srgb(0.165, 0.165, 0.180);
/// Inactive toolbar button background (#2A2A2E)
pub const TOOLBAR_BUTTON_BG: Color = Color::srgb(0.165, 0.165, 0.180);
/// General background color (for widgets)
pub const BACKGROUND_COLOR: Srgba = tailwind::ZINC_800;

// ---------------------------------------------------------------------------
// Component card backgrounds (inspector)
// ---------------------------------------------------------------------------

/// Component card body (#2A2A2E)
pub const COMPONENT_CARD_BG: Color = Color::srgb(0.165, 0.165, 0.180);
/// Component card header bar (#36373B)
pub const COMPONENT_CARD_HEADER_BG: Color = Color::srgb(0.212, 0.216, 0.231);
/// Component card border (#414142)
pub const COMPONENT_CARD_BORDER: Color = Color::srgb(0.255, 0.255, 0.259);

// ---------------------------------------------------------------------------
// Panel / tab styling
// ---------------------------------------------------------------------------

/// Panel header border (#303030)
pub const PANEL_BORDER: Color = Color::srgb(0.188, 0.188, 0.188);
/// Active tab background (#2A2A2E)
pub const TAB_ACTIVE_BG: Color = Color::srgb(0.165, 0.165, 0.180);
/// Active tab top border accent (#206EC8)
pub const TAB_ACTIVE_BORDER: Color = Color::srgb(0.126, 0.431, 0.784);
/// Inactive tab text (#A8A8A8)
pub const TAB_INACTIVE_TEXT: Color = Color::srgb(0.659, 0.659, 0.659);
/// The base color (full alpha) for the drop overlay for draggable tabs
pub const DROP_OVERLAY_BASE: Color = Color::srgb(0.126, 0.431, 0.784);

// ---------------------------------------------------------------------------
// Document tab strip (top-level header tabs; Figma spec)
// ---------------------------------------------------------------------------

/// Active document tab background (Figma #46474C)
pub const DOC_TAB_ACTIVE_BG: Color = Color::srgb(0.275, 0.278, 0.298);
/// Active document tab border (Figma rgba(255,255,255,0.05))
pub const DOC_TAB_ACTIVE_BORDER: Color = Color::srgba(1.0, 1.0, 1.0, 0.05);
/// Active document tab label color (Figma #DBDBDB)
pub const DOC_TAB_ACTIVE_LABEL: Color = Color::srgb(0.859, 0.859, 0.859);
/// Inactive document tab label color (Figma #838385)
pub const DOC_TAB_INACTIVE_LABEL: Color = Color::srgb(0.514, 0.514, 0.522);
/// Dirty-marker dot color inside an active tab (Figma #D9D9D9)
pub const DOC_TAB_DIRTY_DOT: Color = Color::srgb(0.851, 0.851, 0.851);
/// Scene tab accent stripe (Figma #206EC9)
pub const DOC_TAB_SCENE_ACCENT: Color = Color::srgb(0.126, 0.431, 0.788);
/// Tool tab accent stripe for things like Schedule Explorer (Figma #FFCA39)
pub const DOC_TAB_TOOL_ACCENT: Color = Color::srgb(1.0, 0.792, 0.224);

/// Background for Scene View dropdown + Play/Pause pills (Figma #36373B)
pub const HEADER_CONTROL_BG: Color = Color::srgb(0.212, 0.216, 0.231);
/// Border for Scene View dropdown + Play/Pause pills (Figma #414142)
pub const HEADER_CONTROL_BORDER: Color = Color::srgb(0.255, 0.255, 0.259);
/// Label color inside the Scene View dropdown (Figma #DADADA)
pub const HEADER_CONTROL_LABEL: Color = Color::srgb(0.855, 0.855, 0.855);

// ---------------------------------------------------------------------------
// Viewport-specific backgrounds
// ---------------------------------------------------------------------------

/// Viewport tab bar (#1F1F24)
pub const VIEWPORT_TAB_BG: Color = Color::srgb(0.122, 0.122, 0.141);
/// Viewport control bar (#2A2A2E)
pub const VIEWPORT_CONTROL_BG: Color = Color::srgb(0.165, 0.165, 0.180);

// ---------------------------------------------------------------------------
// Elevated / input surfaces
// ---------------------------------------------------------------------------

/// Elevated background for inputs and interactive elements (#36373B)
pub const ELEVATED_BG: Color = Color::srgb(0.212, 0.216, 0.231);
/// Axis label container background, lighter than input (#46474C)
pub const AXIS_LABEL_BG: Color = Color::srgb(0.275, 0.278, 0.298);
/// Active toolbar button background (#505050)
pub const TOOLBAR_ACTIVE_BG: Color = Color::srgb(0.314, 0.314, 0.314);

/// Toolbar outer border (#303030, Figma viewport-toolbar spec).
pub const TOOLBAR_BORDER: Color = Color::srgb(0.188, 0.188, 0.188);
/// Total toolbar row height (Figma viewport-toolbar spec).
pub const TOOLBAR_HEIGHT: f32 = 30.0;
/// Gap between adjacent toolbar children (icon buttons, separators).
pub const TOOLBAR_GAP: f32 = 6.0;
/// Toolbar inset on the left edge (Figma viewport-toolbar spec).
pub const TOOLBAR_PADDING_LEFT: f32 = 6.0;
/// Toolbar inset on the right edge (Figma viewport-toolbar spec).
pub const TOOLBAR_PADDING_RIGHT: f32 = 8.0;
/// Toolbar corner radius (top edges only; bottom flat against the viewport).
pub const TOOLBAR_RADIUS: f32 = 6.0;

// ---------------------------------------------------------------------------
// Borders
// ---------------------------------------------------------------------------

/// Subtle border / separator (#414142)
pub const BORDER_SUBTLE: Color = Color::srgb(0.255, 0.255, 0.259);
/// Strong / emphasized border (#525252)
pub const BORDER_STRONG: Color = Color::Srgba(tailwind::ZINC_600);
/// Standard border color (for widgets)
pub const BORDER_COLOR: Srgba = tailwind::ZINC_700;

// ---------------------------------------------------------------------------
// Interaction states
// ---------------------------------------------------------------------------

/// Hovered row / item background (white 8% alpha)
pub const HOVER_BG: Color = Color::srgba(1.0, 1.0, 1.0, 0.08);
/// Selected item background
pub const SELECTED_BG: Color = Color::srgba(0.0, 0.204, 0.431, 1.0);
/// Selected item border (#206EC8)
pub const SELECTED_BORDER: Color = Color::srgb(0.126, 0.431, 0.784);
/// Active / pressed background
pub const ACTIVE_BG: Color = Color::Srgba(tailwind::ZINC_600);
/// Drag-drop target highlight
pub const DROP_TARGET_BG: Color = Color::Srgba(Srgba {
    red: 0.3,
    green: 0.5,
    blue: 0.2,
    alpha: 1.0,
});
/// Drag-drop target border accent
pub const DROP_TARGET_BORDER: Color = Color::Srgba(Srgba {
    red: 0.3,
    green: 0.7,
    blue: 0.4,
    alpha: 1.0,
});
/// Root container drag-drop overlay
pub const CONTAINER_DROP_TARGET_BG: Color = Color::Srgba(Srgba {
    red: 0.2,
    green: 0.3,
    blue: 0.2,
    alpha: 0.3,
});
/// Tree connection line color
pub const CONNECTION_LINE: Color = Color::srgba(1.0, 1.0, 1.0, 0.2);
/// Disabled text color
pub const TEXT_DISABLED: Color = Color::srgba(0.4, 0.4, 0.4, 0.5);

// ---------------------------------------------------------------------------
// Dialog / modal
// ---------------------------------------------------------------------------

/// Dialog backdrop overlay color (40% black)
pub const DIALOG_BACKDROP: Color = Color::srgba(0.0, 0.0, 0.0, 0.4);
/// Default dialog width
pub const DIALOG_WIDTH: f32 = 400.0;
/// Default dialog max height
pub const DIALOG_MAX_HEIGHT: f32 = 500.0;

// ---------------------------------------------------------------------------
// Shadow colors
// ---------------------------------------------------------------------------

/// Component card / element shadow (#000000 88%)
pub const SHADOW_COLOR: Color = Color::srgba(0.0, 0.0, 0.0, 0.88);
/// Light shadow for elevated inputs (#000000 5%)
pub const SHADOW_COLOR_LIGHT: Color = Color::srgba(0.0, 0.0, 0.0, 0.05);

// ---------------------------------------------------------------------------
// Entity category colors (hierarchy icons)
// ---------------------------------------------------------------------------

/// Camera entity dot color (blue)
pub const CATEGORY_CAMERA: Color = Color::srgba(0.286, 0.506, 0.710, 1.0);
/// Light entity dot color (yellow)
pub const CATEGORY_LIGHT: Color = Color::srgba(1.0, 0.882, 0.0, 1.0);
/// Mesh entity dot color (orange/brown)
pub const CATEGORY_MESH: Color = Color::srgba(0.710, 0.537, 0.294, 1.0);
/// Scene root dot color (cyan)
pub const CATEGORY_SCENE: Color = Color::srgba(0.0, 0.667, 0.733, 1.0);
/// Generic entity dot color (green)
pub const CATEGORY_ENTITY: Color = Color::srgba(0.259, 0.725, 0.514, 1.0);

// ---------------------------------------------------------------------------
// Text colors
// ---------------------------------------------------------------------------

/// Primary text (#ECECEC)
pub const TEXT_PRIMARY: Color = Color::srgb(0.925, 0.925, 0.925);
/// Secondary / dimmed text (#A8A8A8)
pub const TEXT_SECONDARY: Color = Color::srgb(0.659, 0.659, 0.659);
/// Tertiary text: breadcrumbs, field values (#C8C8C8)
pub const TEXT_TERTIARY: Color = Color::srgb(0.784, 0.784, 0.784);
/// Accent / link text
pub const TEXT_ACCENT: Color = Color::Srgba(tailwind::BLUE_400);
/// Accent hover, lighter blue
pub const TEXT_ACCENT_HOVER: Color = Color::Srgba(tailwind::BLUE_300);
/// Body text color (widget standard)
pub const TEXT_BODY_COLOR: Srgba = tailwind::ZINC_200;
/// Display text color (bright)
pub const TEXT_DISPLAY_COLOR: Srgba = tailwind::ZINC_50;
/// Muted text color
pub const TEXT_MUTED_COLOR: Srgba = tailwind::ZINC_400;

// ---------------------------------------------------------------------------
// Type-specific field label colors
// ---------------------------------------------------------------------------

/// Numeric (f32/f64/int) field label, green tint
pub const TYPE_NUMERIC: Color = Color::srgb(0.55, 0.78, 0.55);
/// Boolean field label, blue tint
pub const TYPE_BOOL: Color = Color::srgb(0.55, 0.65, 0.85);
/// String field label, orange tint
pub const TYPE_STRING: Color = Color::srgb(0.85, 0.70, 0.45);
/// Entity reference field label, white
pub const TYPE_ENTITY: Color = Color::Srgba(tailwind::ZINC_300);
/// Enum field label, purple tint
pub const TYPE_ENUM: Color = Color::srgb(0.72, 0.55, 0.82);

// ---------------------------------------------------------------------------
// XYZ axis colors (from updated Figma CSS)
// ---------------------------------------------------------------------------

/// X axis color, red (#AB4051)
#[expect(clippy::approx_constant, reason = "Pure chance this is triggered")]
pub const AXIS_X_COLOR: Color = Color::srgb(0.671, 0.251, 0.318);
/// Y axis color, green (#5D8D0A)
pub const AXIS_Y_COLOR: Color = Color::srgb(0.365, 0.553, 0.039);
/// Z axis color, blue (#2160A3)
pub const AXIS_Z_COLOR: Color = Color::srgb(0.129, 0.376, 0.639);
/// W axis color, neutral grey (#808080)
pub const AXIS_W_COLOR: Color = Color::srgb(0.502, 0.502, 0.502);

// ---------------------------------------------------------------------------
// File browser icon colors
// ---------------------------------------------------------------------------

/// Directory icon, warm yellow
pub const DIR_ICON_COLOR: Color = Color::srgb(0.9, 0.8, 0.3);
/// Generic file icon, grey
pub const FILE_ICON_COLOR: Color = Color::Srgba(tailwind::ZINC_400);

// ---------------------------------------------------------------------------
// Typography
// ---------------------------------------------------------------------------

pub const TEXT_SIZE_XS: f32 = 9.0;
pub const TEXT_SIZE_SM: f32 = 11.0;
pub const TEXT_SIZE: f32 = 13.0;
pub const TEXT_SIZE_LG: f32 = 15.0;
pub const TEXT_SIZE_XL: f32 = 18.0;

// Keep old names as aliases for existing code
pub const FONT_XS: f32 = TEXT_SIZE_XS;
pub const FONT_SM: f32 = TEXT_SIZE_SM;
pub const FONT_MD: f32 = TEXT_SIZE;
pub const FONT_LG: f32 = TEXT_SIZE_LG;

/// Truncation width for browser thumbnail filename labels (asset
/// browser, material browser).
pub const THUMB_NAME_MAX_WIDTH: f32 = 60.0;

// ---------------------------------------------------------------------------
// Icon sizes (Lucide frame sizes)
// ---------------------------------------------------------------------------

/// Small icon size, standard Lucide icons (15px frame)
pub const ICON_SM: f32 = 15.0;
/// Medium icon size, sidebar icons (17px)
pub const ICON_MD: f32 = 17.0;
/// Large icon size (24px)
pub const ICON_LG: f32 = 24.0;

// ---------------------------------------------------------------------------
// Spacing
// ---------------------------------------------------------------------------

pub const SPACING_XS: f32 = 2.0;
pub const SPACING_SM: f32 = 4.0;
pub const SPACING_MD: f32 = 8.0;
pub const SPACING_LG: f32 = 12.0;

// ---------------------------------------------------------------------------
// Layout dimensions
// ---------------------------------------------------------------------------

pub const ROW_HEIGHT: f32 = 24.0;
pub const HEADER_HEIGHT: f32 = 28.0;
pub const STATUS_BAR_HEIGHT: f32 = 22.0;
pub const MENU_BAR_HEIGHT: f32 = 28.0;
pub const INPUT_HEIGHT: f32 = 28.0;

/// Panel tab bar height (Figma: 30px)
pub const PANEL_TAB_HEIGHT: f32 = 30.0;
/// Gap between panels in the layout (Figma: 4px)
pub const PANEL_GAP: f32 = 4.0;
/// Component card corner radius (Figma: 5px)
pub const COMPONENT_CARD_RADIUS: f32 = 5.0;
/// Breadcrumb bar height
pub const BREADCRUMB_HEIGHT: f32 = 34.0;
/// Asset browser sidebar width
pub const SIDEBAR_WIDTH: f32 = 30.0;
/// Search input default width
pub const SEARCH_INPUT_WIDTH: f32 = 200.0;

// ---------------------------------------------------------------------------
// Border radii (numeric)
// ---------------------------------------------------------------------------

pub const BORDER_RADIUS_SM: f32 = 3.0;
pub const BORDER_RADIUS_MD: f32 = 4.0;
pub const BORDER_RADIUS_LG: f32 = 5.0;
