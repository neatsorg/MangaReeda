//! ウインドウに収まらないページのスクロール。
//! 表示位置は「ページの中身をどれだけずらして描くか」で表す（正の値で右・下へずれる）。
//! 下を見るには中身を上へずらすので y は減り、左を見るには右へずらすので x は増える。

/// 収まっているかどうかの判定や端の判定に使う誤差
const EPSILON: f32 = 0.5;

/// 1軸ぶんの、表示位置をずらせる範囲
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub min: f32,
    pub max: f32,
}

impl Span {
    /// 中身（content_min..content_max）を、表示領域（view_min..view_max）の中でずらせる範囲。
    /// 収まっているときはずらさない（0 だけ）
    pub fn new(content_min: f32, content_max: f32, view_min: f32, view_max: f32) -> Self {
        if content_max - content_min <= view_max - view_min + EPSILON {
            Self { min: 0.0, max: 0.0 }
        } else {
            Self {
                min: view_max - content_max,
                max: view_min - content_min,
            }
        }
    }

    pub fn clamp(self, value: f32) -> f32 {
        value.clamp(self.min, self.max)
    }
}

/// 表示位置 (x, y)
pub type Position = (f32, f32);

/// 読み始めの位置: 上端で、右開きなら右端、左開きなら左端
pub fn start_position(x: Span, y: Span, right_to_left: bool) -> Position {
    (if right_to_left { x.min } else { x.max }, y.max)
}

/// 読み終わりの位置: 下端で、右開きなら左端、左開きなら右端
pub fn end_position(x: Span, y: Span, right_to_left: bool) -> Position {
    (if right_to_left { x.max } else { x.min }, y.min)
}

/// 読む順に1回分スクロールした位置。まず下へ、下端まで来たら読む方向へ1列ずらして上端に戻る。
/// もう進めない（ページを送るべき）ときは None
pub fn step_forward(
    position: Position,
    x: Span,
    y: Span,
    step: Position,
    right_to_left: bool,
) -> Option<Position> {
    let (px, py) = (x.clamp(position.0), y.clamp(position.1));
    if py > y.min + EPSILON {
        return Some((px, (py - step.1).max(y.min)));
    }
    if right_to_left && px < x.max - EPSILON {
        Some(((px + step.0).min(x.max), y.max))
    } else if !right_to_left && px > x.min + EPSILON {
        Some(((px - step.0).max(x.min), y.max))
    } else {
        None
    }
}

/// step_forward の逆向き。まず上へ、上端まで来たら1列戻って下端へ。もう戻れないときは None
pub fn step_backward(
    position: Position,
    x: Span,
    y: Span,
    step: Position,
    right_to_left: bool,
) -> Option<Position> {
    let (px, py) = (x.clamp(position.0), y.clamp(position.1));
    if py < y.max - EPSILON {
        return Some((px, (py + step.1).min(y.max)));
    }
    if right_to_left && px > x.min + EPSILON {
        Some(((px - step.0).max(x.min), y.min))
    } else if !right_to_left && px < x.max - EPSILON {
        Some(((px + step.0).min(x.max), y.min))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_that_fits_cannot_move() {
        let span = Span::new(10.0, 90.0, 0.0, 100.0);
        assert_eq!(span, Span { min: 0.0, max: 0.0 });
        assert_eq!(
            step_forward((0.0, 0.0), span, span, (80.0, 80.0), true),
            None
        );
        assert_eq!(
            step_backward((0.0, 0.0), span, span, (80.0, 80.0), true),
            None
        );
    }

    #[test]
    fn larger_content_moves_until_its_edges_meet_the_view() {
        // 中身 -50..150 を 0..100 に表示: 左端を合わせると +50、右端を合わせると -50
        let span = Span::new(-50.0, 150.0, 0.0, 100.0);
        assert_eq!(
            span,
            Span {
                min: -50.0,
                max: 50.0
            }
        );
    }

    #[test]
    fn forward_reads_down_then_the_next_column_in_reading_order() {
        // 縦横とも2画面分（それぞれ -50..=50 の範囲）、1回に80ずつ
        let x = Span::new(-50.0, 150.0, 0.0, 100.0);
        let y = x;
        let step = (80.0, 80.0);

        // 右開き: 右上から始めて下へ、次は左の列の上へ
        let mut position = start_position(x, y, true);
        assert_eq!(position, (-50.0, 50.0));
        let mut visited = vec![position];
        while let Some(next) = step_forward(position, x, y, step, true) {
            position = next;
            visited.push(position);
        }
        assert_eq!(
            visited,
            [
                (-50.0, 50.0),
                (-50.0, -30.0),
                (-50.0, -50.0),
                (30.0, 50.0),
                (30.0, -30.0),
                (30.0, -50.0),
                (50.0, 50.0),
                (50.0, -30.0),
                (50.0, -50.0)
            ]
        );
        assert_eq!(position, end_position(x, y, true));

        // 戻るときは逆順にたどって読み始めの位置に戻る
        while let Some(previous) = step_backward(position, x, y, step, true) {
            position = previous;
        }
        assert_eq!(position.0, x.min);
        assert_eq!(position.1, y.max);

        // 左開きは左上から始めて右の列へ
        assert_eq!(start_position(x, y, false), (50.0, 50.0));
        assert_eq!(
            step_forward((50.0, -50.0), x, y, step, false),
            Some((-30.0, 50.0))
        );
        assert_eq!(end_position(x, y, false), (-50.0, -50.0));
    }

    #[test]
    fn positions_outside_the_range_are_pulled_back_first() {
        // ドラッグで範囲外にずらしたあとでも、範囲内に戻してから進める
        let x = Span { min: 0.0, max: 0.0 };
        let y = Span::new(-100.0, 200.0, 0.0, 100.0);
        assert_eq!(
            step_forward((40.0, 500.0), x, y, (80.0, 80.0), true),
            Some((0.0, 20.0))
        );
    }
}
