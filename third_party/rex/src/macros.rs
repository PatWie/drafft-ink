#![allow(unused_macros)]

// --------------------
// Static maps
// --------------------

macro_rules! map {
    ($($key:expr => $val:expr,)*) => {
        &[$(($key, $val)),*]
    };
}

// ----------------------
// Parsing related Macros
// ----------------------

macro_rules! alt {
    ($e:expr $(,)*) => (
        match $e? {
            Some(res) => Some(res),
            None => None,
        }
    );

    ($e:expr, $($tt:tt)*) => (
        match $e? {
            Some(res) => Some(res),
            None => {
                alt!($($tt)*)
            }
        }
    )
}

// ----------------------
// Testing related Macros
// ----------------------

macro_rules! assert_close {
    ($x:expr, $y:expr, $epsilon:expr) => {
        {
            let (x, y, epsilon) = ($x, $y, $epsilon);
            assert!(
                (x - y).abs() <= epsilon,
                "Assertion failed: `abs(left - right) <= epsilon`, with `left` = {:?}, `right` = {:?}, `epsilon` = {:?}",
                x,
                y,
                epsilon
            );
        }
    };
}

macro_rules! should_fail {
    ($errs:ident, $func:ident, $iter:expr) => ({
        for item in $iter.iter() {
            if let Ok(_) = $func(item) {
                $errs.push(format!("{:?} - should have errored.\n", item));
            }
        }
    })
}

macro_rules! should_pass {
    ($errs:ident, $func:ident, $iter:expr) => ({
        for item in $iter.iter() {
            if let Err(s) = $func(item) {
                $errs.push(format!("{:?} - should have passed.\n\tError: {:?}\n", item, s));
            }
        }
    })
}

macro_rules! should_equate {
    ($errs:ident, $func:ident, $iter:expr) => ({
        for &(l, r) in $iter.iter() {
            let l_res = $func(l);
            let r_res = $func(r);
            if l_res != r_res {
                $errs.push(format!("{:?} and {:?} - should have yielded the same results.\n\n\tLeft:  {:?}\n\n\tRight: {:?}",
                    l, r, l_res, r_res));
            }
        }
    })
}

macro_rules! should_differ {
    ($errs:ident, $func:ident, $iter:expr) => ({
        for &(l, r) in $iter.iter() {
            let l_res = $func(l);
            let r_res = $func(r);
            if l_res == r_res {
                $errs.push(format!("{:?} and {:?} - should have yielded different results.\n\n\tLeft:  {:?}\n\n\tRight: {:?}",
                    l, r, l_res, r_res));
            }
        }
    })
}

macro_rules! display_errors {
    ($errs:ident) => (
        if $errs.len() > 0 {
            for err in $errs {
                println!("\n{}", err);
            }
            panic!();
        }
    )
}

// These max/min macros were borrowed
// from the max_min_macros crate by Emanuel Claesson

macro_rules! min {
    ($x: expr) => ($x);
    ($x: expr, $($xs: expr), +) => {
        {
            Unit::min($x, min!($($xs), +))
        }
    }
}

macro_rules! max {
    ($x: expr) => ($x);
    ($x: expr, $($xs: expr), +) => {
        {
            Unit::max($x, max!($($xs), +))
        }
    }
}
