//! ルーペの形の計算。
//! ルーペは中心のまわりを倍率 m で拡大して見せる円で、ルーペ内の点 q には、
//! 元の画面の点 center + (q - center) / m が映る。

pub type Point = [f32; 2];

/// 円を近似する多角形（反時計回りとは限らないが凸）
pub fn circle(center: Point, radius: f32, segments: usize) -> Vec<Point> {
    (0..segments)
        .map(|i| {
            let angle = i as f32 / segments as f32 * std::f32::consts::TAU;
            [
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            ]
        })
        .collect()
}

/// ルーペ内の点 q に映る、元の画面の点
pub fn source_point(center: Point, q: Point, magnification: f32) -> Point {
    [
        center[0] + (q[0] - center[0]) / magnification,
        center[1] + (q[1] - center[1]) / magnification,
    ]
}

/// 元の画面の点 p が、ルーペ内で映る点（source_point の逆）
pub fn loupe_point(center: Point, p: Point, magnification: f32) -> Point {
    [
        center[0] + (p[0] - center[0]) * magnification,
        center[1] + (p[1] - center[1]) * magnification,
    ]
}

/// 凸多角形を、軸に平行な長方形 min..max で切り抜く（Sutherland–Hodgman 法）。
/// 重ならなければ空になる
pub fn clip_to_rect(polygon: &[Point], min: Point, max: Point) -> Vec<Point> {
    let mut result = polygon.to_vec();
    // (軸, 境界の値, 境界の内側が大きい側か)
    for (axis, bound, keep_greater) in [
        (0, min[0], true),
        (0, max[0], false),
        (1, min[1], true),
        (1, max[1], false),
    ] {
        let inside = |p: &Point| {
            if keep_greater {
                p[axis] >= bound
            } else {
                p[axis] <= bound
            }
        };
        let input = std::mem::take(&mut result);
        for (i, current) in input.iter().enumerate() {
            let previous = &input[(i + input.len() - 1) % input.len()];
            if inside(current) != inside(previous) {
                let t = (bound - previous[axis]) / (current[axis] - previous[axis]);
                let other = 1 - axis;
                let mut crossing = [0.0; 2];
                crossing[axis] = bound;
                crossing[other] = previous[other] + t * (current[other] - previous[other]);
                result.push(crossing);
            }
            if inside(current) {
                result.push(*current);
            }
        }
        if result.is_empty() {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(polygon: &[Point]) -> (Point, Point) {
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for p in polygon {
            for axis in 0..2 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        (min, max)
    }

    #[test]
    fn source_and_loupe_points_are_inverse() {
        let center = [100.0, 50.0];
        let q = [130.0, 20.0];
        assert_eq!(source_point(center, q, 2.0), [115.0, 35.0]);
        assert_eq!(loupe_point(center, source_point(center, q, 2.0), 2.0), q);
    }

    #[test]
    fn a_rect_containing_the_circle_keeps_it_whole() {
        let circle = circle([0.0, 0.0], 10.0, 32);
        assert_eq!(clip_to_rect(&circle, [-20.0, -20.0], [20.0, 20.0]), circle);
    }

    #[test]
    fn clipping_cuts_the_circle_at_the_rect_edges() {
        let circle = circle([0.0, 0.0], 10.0, 32);
        // 右半分だけを残す
        let clipped = clip_to_rect(&circle, [0.0, -20.0], [20.0, 20.0]);
        let (min, max) = bounds(&clipped);
        assert_eq!(min[0], 0.0);
        assert!((max[0] - 10.0).abs() < 1e-4);
        assert!((min[1] + 10.0).abs() < 1e-4 && (max[1] - 10.0).abs() < 1e-4);
        assert!(clipped.iter().all(|p| p[0] >= 0.0));
    }

    #[test]
    fn a_rect_outside_the_circle_leaves_nothing() {
        let circle = circle([0.0, 0.0], 10.0, 32);
        assert!(clip_to_rect(&circle, [30.0, 30.0], [40.0, 40.0]).is_empty());
    }
}
