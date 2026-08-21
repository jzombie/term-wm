//! All tunable constants for opencar live here (repo convention: no inline
//! magic numbers). Units are meters / seconds / radians unless noted.

// ── Simulation loop ─────────────────────────────────────────────────────
pub const SIM_TICK_HZ: f32 = 60.0;
pub const SIM_TICK_DT: f32 = 1.0 / SIM_TICK_HZ;
pub const MAX_FRAME_SECS: f32 = 0.05;
pub const MAX_SIM_SUBSTEPS: u32 = 5;
pub const EVENT_POLL_MILLIS: u64 = 8;

// ── World generation ────────────────────────────────────────────────────
pub const CHUNK_SIZE_I32: i32 = 64;
pub const CHUNK_LOAD_RADIUS: i32 = 3;
pub const CHUNK_GEN_BUDGET_PER_FRAME: usize = 2;

pub const BASE_ELEV: f32 = 8.0;
pub const CONTINENT_SCALE: f32 = 1350.0;
pub const CONTINENT_LOW: f32 = 0.52;
pub const CONTINENT_HIGH: f32 = 0.74;
pub const RIDGE_SCALE: f32 = 255.0;
pub const HILL_SCALE: f32 = 110.0;
pub const HILL_AMP: f32 = 30.0;
pub const MOUNTAIN_AMP: f32 = 240.0;
pub const SEA_LEVEL: f32 = 5.0;
pub const SAND_BAND: f32 = 1.1;
pub const SNOW_LINE: f32 = 155.0;
pub const SNOW_DITHER_AMP: f32 = 14.0;
pub const ROCK_SLOPE: f32 = 0.85;
pub const ROCK_BAND_ELEV: f32 = 118.0;
pub const SLOPE_SAMPLE_STEP: f32 = 1.5;
pub const MOISTURE_SCALE: f32 = 210.0;
pub const WATER_SHALLOW_DEPTH: f32 = 1.6;

// ── Roads (analytic highway network) ────────────────────────────────────
pub const ROAD_HALF_WIDTH: f32 = 4.6;
pub const SHOULDER_WIDTH: f32 = 1.6;
pub const BLEND_DIST: f32 = 26.0;
pub const EDGE_LINE_INNER: f32 = 4.05;
pub const EDGE_LINE_OUTER: f32 = 4.45;
pub const DASH_HALF_WIDTH: f32 = 0.22;
pub const DASH_PERIOD: f32 = 9.0;
pub const DASH_DUTY: f32 = 0.45;

pub const WAVE1_LEN: f32 = 900.0;
pub const WAVE1_AMP: f32 = 55.0;
pub const WAVE2_LEN: f32 = 215.0;
pub const WAVE2_AMP: f32 = 18.0;
pub const ROAD_WARP_AMP: f32 = 110.0;
pub const ROAD_WARP_SCALE: f32 = 640.0;
pub const SLOPE_EPS: f32 = 0.8;

pub const ROAD_BASE_ELEV: f32 = 13.0;
pub const ELEV1_LEN: f32 = 380.0;
pub const ELEV1_AMP: f32 = 16.0;
pub const ELEV2_LEN: f32 = 95.0;
pub const ELEV2_AMP: f32 = 6.0;

pub const CURVE_SIGN_KAPPA: f32 = 0.0045;
pub const SIGN_SPACING: f32 = 60.0;
pub const SIGN_MARGIN: f32 = 48.0;
pub const SIGN_LATERAL: f32 = ROAD_HALF_WIDTH + 2.2;

pub const RAIL_DIST_IN: f32 = ROAD_HALF_WIDTH + 0.3;
pub const RAIL_DIST_OUT: f32 = ROAD_HALF_WIDTH + 0.65;
pub const RAIL_LOOK_AHEAD: f32 = 3.5;
pub const RAIL_DROP_THRESH: f32 = 2.4;

// ── Junctions (explicit height grids — O(1) queries) ────────────────────
pub const JUNCTION_HALF: f32 = 11.0;
pub const JUNCTION_GRID_DIM: usize = 33;
pub const JUNCTION_BLEND: f32 = 16.0;

// ── Traffic AI ──────────────────────────────────────────────────────────
pub const NPC_COUNT: usize = 18;
pub const NPC_CRUISE_MIN: f32 = 17.0;
pub const NPC_CRUISE_MAX: f32 = 31.0;
/// Lane center offsets from the centerline, right of +t travel.
pub const LANE_OFFSETS: [f32; 2] = [1.8, 5.4];
pub const NPC_ACCEL: f32 = 2.2;
pub const NPC_BRAKE: f32 = 5.0;
pub const GAP_SCAN: f32 = 70.0;
pub const MIN_GAP: f32 = 14.0;
pub const RECYCLE_BEHIND: f32 = 340.0;
pub const RECYCLE_AHEAD: f32 = 720.0;
pub const SPAWN_MIN: f32 = 240.0;
pub const SPAWN_MAX: f32 = 660.0;
pub const SPAWN_STRIDE: f32 = 25.0;
pub const SPAWN_SCAN_RANGE: f32 = 900.0;
/// Bezier turn arcs through junctions take roughly this long.
pub const JUNCTION_TURN_SPEED: f32 = 9.0;

// ── Player physics (kinematic arcade) ───────────────────────────────────
pub const MAX_SPEED: f32 = 50.0; // 180 km/h
pub const MAX_REVERSE: f32 = 9.0;
pub const ENGINE_ACCEL: f32 = 4.4;
pub const BRAKE_DECEL: f32 = 9.0;
pub const HANDBRAKE_DECEL: f32 = 13.0;
pub const DRAG_COEFF: f32 = 0.0026;
pub const ROLL_RESIST: f32 = 0.09;
pub const OFFROAD_DRAG_MULT: f32 = 7.0;
pub const OFFROAD_MAX_SPEED: f32 = 17.0;
pub const STEER_RATE: f32 = 1.7; // rad/s at reference speed
/// Gentle pull toward the lane tangent while on asphalt (rad/s max).
pub const LANE_MAGNETISM: f32 = 0.026;
pub const STEER_SMOOTH_RATE: f32 = 6.0; // input slew per second
pub const LOW_SPEED_REF: f32 = 10.0; // full steering authority below this
pub const HIGH_SPEED_STABILITY: f32 = 0.62; // steering retained at top speed
pub const COLLIDE_RADIUS: f32 = 2.4;
/// Longitudinal acceleration used by the camera pitch response.
pub const PITCH_RESPONSE: f32 = 0.010; // rad per m/s²

// ── Chase camera & chassis dynamics ─────────────────────────────────────
/// (back distance, height) presets cycled with `C`.
pub const CAM_PRESETS: [(f32, f32); 3] = [(6.8, 2.7), (10.6, 4.3), (4.4, 1.9)];
pub const CAM_SPRING_XZ: f32 = 7.0;
pub const CAM_Y_SPRING: f32 = 9.0;
pub const CAM_YAW_RATE: f32 = 4.6;
pub const CAM_SPEED_PULLBACK: f32 = 0.07;
pub const CAM_MIN_CLEARANCE: f32 = 0.7;
/// Cornering roll gain: roll = ROLL_GAIN * steer_sm * speed.
pub const ROLL_GAIN: f32 = 0.0035;
pub const ROLL_MAX: f32 = 0.35;
pub const ROLL_EPS: f32 = 0.004;
/// Weight-transfer pitch from longitudinal accel.
pub const CAM_PITCH_GAIN: f32 = PITCH_RESPONSE;

// ── Rendering ───────────────────────────────────────────────────────────
pub const VIEW_NEAR: f32 = 4.0;
pub const VIEW_FAR: f32 = 1400.0;
pub const STEP_BASE: f32 = 0.4;
pub const STEP_GROWTH: f32 = 0.008;
pub const FOV_H_DEG: f32 = 74.0;
/// Base downward pitch so the road fills the lower frame.
pub const CAM_PITCH_BASE: f32 = -0.055;
/// Terrain-slope contribution to camera pitch.
pub const SLOPE_PITCH_GAIN: f32 = 0.6;
/// Clamp for combined slope+dive pitch (radians).
pub const CAM_PITCH_LIMIT: f32 = 0.45;
/// Look-ahead/behind distance for slope sampling (one car-length).
pub const SLOPE_LOOK: f32 = 2.2;
/// Slight negative barrel distortion coefficient of the virtual lens.
/// Slight negative barrel distortion, BOUNDED at startup by
/// `LENS_MAX_EDGE_CELLS` (see render::lens).
pub const LENS_K: f32 = 0.0;
/// Hard cap: distortion displacement at the screen edge ≤ this many cells.
pub const LENS_MAX_EDGE_CELLS: f32 = 1.0;
/// Terrain z-writes are pushed back this far so meshes clear the voxel
/// plane (terrain-side only — mesh depths stay exact).
pub const TERRAIN_DEPTH_MARGIN: f32 = 0.30;
/// Continuous near-field filter: spread = clamp(K / fwd_d, MIN, MAX).
pub const FILTER_NEAR_K: f32 = 1.6;
pub const FILTER_MIN_SPREAD: f32 = 0.06;
pub const FILTER_MAX_SPREAD: f32 = 0.5;
/// Brightness floor inside shadows (multiplicative on lit color).
pub const SHADOW_MIN_LIGHT: f32 = 0.42;
/// Per-channel sample spread below which a cell counts as "flat" and uses
/// temporal fractional-coverage halftone instead of edge thresholding.
pub const FLAT_RANGE_EPS: f32 = 12.0;
/// Minimum halftone coverage for flat cells — shadowed terrain keeps a
/// sparse 1–2 dot pattern instead of collapsing to empty glyphs.
pub const LUMA_FLOOR_COVERAGE: f32 = 0.12;
/// Cells whose mean luma is below this are allowed to go fully dark.
pub const DARK_CELL_FLOOR_LUMA: f32 = 16.0;
/// Distance-scaled boost for thin bright features (lane paint, rails) so
/// they stay above the quantization threshold toward the horizon.
pub const DETAIL_BOOST_GAIN: f32 = 0.9;
pub const DETAIL_NEAR_M: f32 = 40.0;
pub const DETAIL_FAR_M: f32 = 420.0;
/// Only features at least this bright get the distance boost.
pub const DETAIL_LUMA_MIN: f32 = 170.0;
/// Epsilon guarding the Sobel depth-normalization division.
pub const SOBEL_EPSILON: f32 = 0.001;
/// Distance-normalized gradient threshold for edge contours.
pub const EDGE_DEPTH_GRAD: f32 = 0.18;
/// Off-road roll cap (radians) — keeps frustum within overscan capacity.
pub const ROLL_OFFROAD_MAX: f32 = 0.2618; // 15°
/// Lens bypass threshold: chassis vibration above this disables the
/// radial remap for the frame (discrete cells can't resolve it in motion).
pub const SHAKE_BYPASS_M: f32 = 0.01;
/// Additive self-light on vehicle bodies so cars read as solid shapes.
pub const SELF_LIGHT: f32 = 0.45;
/// Edge contours are restricted to near/mid-field geometry.
pub const EDGE_MAX_Z: f32 = 220.0;
/// Near plane for mesh clipping (camera space).
pub const Z_NEAR: f32 = 0.1;
pub const FOG_DIST: f32 = 470.0;
pub const AMBIENT: f32 = 0.38;
pub const DIFFUSE: f32 = 0.75;
/// Direction from a surface toward the sun (normalized).
pub const SUN_DIR: [f32; 3] = [-0.46, 0.80, -0.33];
pub const NORMAL_EPS: f32 = 1.6;
pub const WATER_MIRROR_TERRAIN: f32 = 0.45;
pub const WATER_MIRROR_SKY: f32 = 0.55;
/// Sky gradient stops as fractions of the horizon height.
pub const SKY_SPLIT_HIGH: f32 = 0.45;
pub const SKY_SPLIT_LOW: f32 = 0.82;
pub const CLOUD_BAND_CENTER: f32 = 0.62;
pub const CLOUD_BAND_HALF: f32 = 0.16;
pub const CLOUD_THRESHOLD: f32 = 0.56;
pub const CLOUD_ANGULAR_FREQ: f32 = 6.0;
pub const CLOUD_DRIFT: f32 = 0.02;
pub const SUN_BEARING: f32 = -0.9; // radians left of north
pub const SUN_RADIUS_PX_SCALE: f32 = 0.03; // fraction of pixel width
pub const SUN_ELEVATION_ROW_FRAC: f32 = 0.72;

// Sprites/props draw distance.
pub const SPRITE_FAR: f32 = 600.0;

// ── Shadows ─────────────────────────────────────────────────────────────
pub const SHADOW_MAP_DIM: usize = 256;
pub const SHADOW_COVERAGE_M: f32 = 30.0;
/// Lift of decal shadow quads above ground along the surface normal.
pub const SHADOW_EPS: f32 = 0.03;
/// Normal-offset bias against shadow-map acne (meters along normal).
pub const SHADOW_BIAS: f32 = 0.15;
/// Poisson-disk tap offsets for percentage-closer filtering (x, y pairs).
pub const PCF_TAPS: [[f32; 2]; 4] = [
    [-0.62, -0.31],
    [0.41, -0.56],
    [-0.11, 0.64],
    [0.58, 0.38],
];
pub const PCF_TAP_COUNT: usize = PCF_TAPS.len();

// ── Quantization / grain / lens / display ───────────────────────────────
/// Floor on the centroid-separation needed for a genuine two-color split;
/// the effective threshold scales up with brightness to cover grain jitter
/// (`2 * DITHER_AMP * luma`), so flat blocks collapse instead of sparkling.
pub const CENTROID_SEPARATION_MIN: f32 = 6.0;
pub const DITHER_AMP: f32 = 16.0;
pub const NOISE_TABLE_DIM: usize = 64;
pub const KMEANS_ITERS: usize = 4;
/// Encoder sampling aspect. The render buffer is built isotropic at 2×4 px
/// per cell, so nominal targets use IDENTITY — direct integer indexing.
pub const DEFAULT_CELL_ASPECT: f32 = 1.0;
pub const MIN_CELLS_W: u16 = 48;
pub const MIN_CELLS_H: u16 = 16;
pub const MINIMAP_COLS: u16 = 20;
pub const MINIMAP_ROWS: u16 = 8;

// ── Input ───────────────────────────────────────────────────────────────
/// Held-key heartbeat timeout when the terminal lacks key-release events.
/// Must exceed the worst-case OS initial auto-repeat delay (~500 ms).
pub const FALLBACK_HELD_TIMEOUT_SECS: f32 = 0.6;
pub const BOB_PHASE_RATE: f32 = 0.55; // bob phase per meter traveled
pub const OFFROAD_BOB_MULT: f32 = 3.0;

// ── Palette (index shared by materials and meshes) ──────────────────────
pub const PAL_SKY_TOP: u8 = 0;
pub const PAL_SKY_HIGH: u8 = 1;
pub const PAL_SKY_LOW: u8 = 2;
pub const PAL_SKY_HORIZON: u8 = 3;
pub const PAL_SUN: u8 = 4;
pub const PAL_CLOUD: u8 = 5;
pub const PAL_WATER: u8 = 6;
pub const PAL_SHALLOW: u8 = 7;
pub const PAL_SAND: u8 = 8;
pub const PAL_GRASS: u8 = 9;
pub const PAL_GRASS_DARK: u8 = 10;
pub const PAL_GRASS_DRY: u8 = 11;
pub const PAL_DIRT: u8 = 12;
pub const PAL_ROCK: u8 = 13;
pub const PAL_ROCK_LIGHT: u8 = 14;
pub const PAL_SNOW: u8 = 15;
pub const PAL_ASPHALT: u8 = 16;
pub const PAL_ASPHALT_WORN: u8 = 17;
pub const PAL_SHOULDER: u8 = 18;
pub const PAL_PAINT: u8 = 19;
pub const PAL_RAIL: u8 = 20;
pub const PAL_CAR_RED: u8 = 21;
pub const PAL_CAR_WHITE: u8 = 22;
pub const PAL_CAR_BLUE: u8 = 23;
pub const PAL_CAR_DARK: u8 = 24;
pub const PAL_GLASS: u8 = 25;
pub const PAL_TIRE: u8 = 26;
pub const PAL_TAIL: u8 = 27;
pub const PAL_HEAD: u8 = 28;
pub const PAL_SIGN_YELLOW: u8 = 29;
pub const PAL_SIGN_BLACK: u8 = 30;
pub const PAL_POLE: u8 = 31;

pub const PALETTE: [[u8; 3]; 32] = [
    [36, 58, 108],    // SKY_TOP
    [86, 118, 172],   // SKY_HIGH
    [152, 176, 212],  // SKY_LOW
    [196, 206, 222],  // SKY_HORIZON
    [255, 244, 198],  // SUN
    [214, 222, 236],  // CLOUD
    [34, 72, 124],    // WATER
    [58, 104, 148],   // SHALLOW
    [198, 182, 134],  // SAND
    [74, 124, 60],    // GRASS
    [52, 98, 46],     // GRASS_DARK
    [128, 138, 70],   // GRASS_DRY
    [128, 100, 64],   // DIRT
    [126, 124, 118],  // ROCK
    [162, 158, 150],  // ROCK_LIGHT
    [236, 240, 248],  // SNOW
    [44, 44, 48],     // ASPHALT
    [66, 66, 70],     // ASPHALT_WORN
    [86, 84, 84],     // SHOULDER
    [242, 242, 238],  // PAINT
    [188, 196, 208],  // RAIL
    [198, 44, 44],    // CAR_RED
    [228, 230, 236],  // CAR_WHITE
    [52, 92, 196],    // CAR_BLUE
    [40, 42, 48],     // CAR_DARK
    [70, 88, 110],    // GLASS
    [24, 24, 26],     // TIRE
    [255, 64, 40],    // TAIL
    [255, 238, 180],  // HEAD
    [248, 208, 64],   // SIGN_YELLOW
    [30, 30, 32],     // SIGN_BLACK
    [120, 124, 130],  // POLE
];

// Terrain material ids double as palette indices.
pub const MAT_WATER: u8 = PAL_WATER;
pub const MAT_SHALLOW: u8 = PAL_SHALLOW;
pub const MAT_SAND: u8 = PAL_SAND;
pub const MAT_GRASS: u8 = PAL_GRASS;
pub const MAT_GRASS_DARK: u8 = PAL_GRASS_DARK;
pub const MAT_GRASS_DRY: u8 = PAL_GRASS_DRY;
pub const MAT_DIRT: u8 = PAL_DIRT;
pub const MAT_ROCK: u8 = PAL_ROCK;
pub const MAT_ROCK_LIGHT: u8 = PAL_ROCK_LIGHT;
pub const MAT_SNOW: u8 = PAL_SNOW;
pub const MAT_ASPHALT: u8 = PAL_ASPHALT;
pub const MAT_ASPHALT_WORN: u8 = PAL_ASPHALT_WORN;
pub const MAT_SHOULDER: u8 = PAL_SHOULDER;
pub const MAT_PAINT: u8 = PAL_PAINT;
pub const MAT_RAIL: u8 = PAL_RAIL;

/// Surfaces the car can drive on without an off-road penalty.
pub fn is_drivable_surface(mat: u8) -> bool {
    matches!(
        mat,
        MAT_ASPHALT | MAT_ASPHALT_WORN | MAT_SHOULDER | MAT_PAINT
    )
}
