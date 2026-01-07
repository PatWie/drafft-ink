//! Elbow arrow routing using A* pathfinding.
//!
//! Creates right-angle paths between two points with minimal turns.
//! Uses departure/arrival waypoints to ensure clean entry/exit angles.

use kurbo::Point;
use pathfinding::prelude::astar;

const GRID_SIZE: f64 = 20.0;

fn to_grid(v: f64) -> i32 {
    (v / GRID_SIZE).round() as i32
}

fn from_grid(v: i32) -> f64 {
    v as f64 * GRID_SIZE
}

/// Cardinal direction for orthogonal movement.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Heading {
    Up,
    Down,
    Left,
    Right,
    None,
}

impl Heading {
    fn reverse(&self) -> Heading {
        match self {
            Heading::Up => Heading::Down,
            Heading::Down => Heading::Up,
            Heading::Left => Heading::Right,
            Heading::Right => Heading::Left,
            Heading::None => Heading::None,
        }
    }
}

/// Grid cell with current heading for A* state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Cell {
    x: i32,
    y: i32,
    heading: Heading,
}

impl Cell {
    fn new(x: i32, y: i32, heading: Heading) -> Self {
        Self { x, y, heading }
    }
}

fn manhattan(x1: i32, y1: i32, x2: i32, y2: i32) -> u64 {
    ((x1 - x2).abs() + (y1 - y2).abs()) as u64
}

/// Compute elbow path between two points.
/// Returns intermediate corner points (not including start and end).
pub fn compute_elbow_path(start: Point, end: Point) -> Vec<Point> {
    let dx = end.x - start.x;
    let dy = end.y - start.y;

    // Perfectly aligned - no intermediate points needed
    if dx.abs() < GRID_SIZE {
        return vec![]; // Vertical line
    }
    if dy.abs() < GRID_SIZE {
        return vec![]; // Horizontal line
    }

    // Departure heading: direction from start toward end (prefer horizontal)
    let departure_heading = if dx.abs() >= dy.abs() {
        if dx > 0.0 {
            Heading::Right
        } else {
            Heading::Left
        }
    } else if dy > 0.0 {
        Heading::Down
    } else {
        Heading::Up
    };

    // Dongles at midpoint on the departure axis (like Excalidraw's dynamic bounds)
    let mid_x = (start.x + end.x) / 2.0;
    let mid_y = (start.y + end.y) / 2.0;

    let (departure, arrival) = match departure_heading {
        Heading::Right | Heading::Left => {
            // Horizontal departure: dongles at mid_x
            (Point::new(mid_x, start.y), Point::new(mid_x, end.y))
        }
        _ => {
            // Vertical departure: dongles at mid_y
            (Point::new(start.x, mid_y), Point::new(end.x, mid_y))
        }
    };

    let sx = to_grid(departure.x);
    let sy = to_grid(departure.y);
    let ex = to_grid(arrival.x);
    let ey = to_grid(arrival.y);

    // If waypoints are aligned, just return them
    if sx == ex || sy == ey {
        return vec![departure, arrival];
    }

    let turn_penalty = manhattan(sx, sy, ex, ey);
    let start_cell = Cell::new(sx, sy, departure_heading);

    let (path, _) = astar(
        &start_cell,
        |cell| neighbors(cell, turn_penalty),
        |cell| estimate(cell, ex, ey, turn_penalty),
        |cell| cell.x == ex && cell.y == ey,
    )
    .expect("A* always finds a path on unbounded grid");

    // Build result: departure + corners + arrival
    let mut result = vec![departure];
    result.extend(extract_corners(&path, departure, arrival));
    result.push(arrival);

    result
}

fn neighbors(cell: &Cell, turn_penalty: u64) -> Vec<(Cell, u64)> {
    let moves = [
        (0, -1, Heading::Up),
        (0, 1, Heading::Down),
        (-1, 0, Heading::Left),
        (1, 0, Heading::Right),
    ];

    moves
        .iter()
        .filter(|(_, _, h)| *h != cell.heading.reverse())
        .map(|(dx, dy, h)| {
            let cost = if cell.heading == Heading::None || cell.heading == *h {
                1
            } else {
                1 + turn_penalty.pow(3)
            };
            (Cell::new(cell.x + dx, cell.y + dy, *h), cost)
        })
        .collect()
}

fn estimate(cell: &Cell, ex: i32, ey: i32, turn_penalty: u64) -> u64 {
    let dist = manhattan(cell.x, cell.y, ex, ey);
    let turns = if cell.x == ex || cell.y == ey { 0 } else { 1 };
    dist + turns * turn_penalty.pow(2)
}

fn extract_corners(path: &[Cell], start: Point, end: Point) -> Vec<Point> {
    let mut corners = Vec::new();

    for i in 1..path.len() - 1 {
        if path[i - 1].heading != path[i].heading && path[i - 1].heading != Heading::None {
            let mut corner = Point::new(from_grid(path[i].x), from_grid(path[i].y));

            // Snap to waypoint coordinates for cleaner lines
            if path[i].x == path[0].x {
                corner.x = start.x;
            }
            if path[i].y == path[0].y {
                corner.y = start.y;
            }
            if path[i].x == path.last().unwrap().x {
                corner.x = end.x;
            }
            if path[i].y == path.last().unwrap().y {
                corner.y = end.y;
            }

            corners.push(corner);
        }
    }

    corners
}
