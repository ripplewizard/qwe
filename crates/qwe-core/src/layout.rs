use serde::{Deserialize, Serialize};
use std::str::FromStr;
use thiserror::Error;

use crate::PaneId;

pub const MAX_LAYOUT_PANES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    Normal,
    Floating,
}

impl PaneKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Floating => "floating",
        }
    }

    #[must_use]
    pub fn from_db(value: &str) -> Self {
        match value {
            "floating" => Self::Floating,
            _ => Self::Normal,
        }
    }
}

impl FromStr for PaneKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "normal" => Ok(Self::Normal),
            "floating" => Ok(Self::Floating),
            _ => Err(format!("unknown Pane kind '{value}'")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutRect {
    pub x: u16,
    pub y: u16,
    pub rows: u16,
    pub columns: u16,
}

impl LayoutRect {
    pub fn validate(self) -> Result<(), LayoutError> {
        if self.rows == 0 || self.columns == 0 {
            return Err(LayoutError::InvalidViewport {
                rows: self.rows,
                columns: self.columns,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneLayout {
    pub pane_id: PaneId,
    pub kind: PaneKind,
    pub layout_order: u32,
    pub layout_weight: u16,
    pub z_index: u32,
    pub focused: bool,
    pub rect: LayoutRect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSpec {
    pub pane_id: PaneId,
    pub kind: PaneKind,
    pub layout_order: u32,
    pub layout_weight: u16,
    pub z_index: u32,
    pub floating_rect: Option<LayoutRect>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LayoutError {
    #[error("a Session layout requires at least one Pane")]
    NoPanes,
    #[error("a Session layout supports at most {maximum} Panes, received {received}")]
    TooManyPanes { received: usize, maximum: usize },
    #[error("terminal viewport must have positive dimensions, received {rows}x{columns}")]
    InvalidViewport { rows: u16, columns: u16 },
}

pub fn tile_panes(
    panes: &[PaneId],
    rows: u16,
    columns: u16,
    focused: Option<&PaneId>,
) -> Result<Vec<PaneLayout>, LayoutError> {
    let specs = panes
        .iter()
        .enumerate()
        .map(|(index, pane_id)| PaneSpec {
            pane_id: pane_id.clone(),
            kind: PaneKind::Normal,
            layout_order: index as u32,
            layout_weight: 1,
            z_index: 0,
            floating_rect: None,
        })
        .collect::<Vec<_>>();
    tile_pane_specs(&specs, rows, columns, focused)
}

pub fn tile_pane_specs(
    panes: &[PaneSpec],
    rows: u16,
    columns: u16,
    focused: Option<&PaneId>,
) -> Result<Vec<PaneLayout>, LayoutError> {
    if panes.is_empty() {
        return Err(LayoutError::NoPanes);
    }
    if panes.len() > MAX_LAYOUT_PANES {
        return Err(LayoutError::TooManyPanes {
            received: panes.len(),
            maximum: MAX_LAYOUT_PANES,
        });
    }
    if rows == 0 || columns == 0 {
        return Err(LayoutError::InvalidViewport { rows, columns });
    }

    let mut tiled = panes
        .iter()
        .filter(|pane| pane.kind == PaneKind::Normal)
        .collect::<Vec<_>>();
    tiled.sort_by_key(|pane| (pane.layout_order, pane.pane_id.to_string()));
    let grid_columns = (tiled.len() as f64).sqrt().ceil().max(1.0) as usize;
    let grid_rows = tiled.len().div_ceil(grid_columns).max(1);
    let mut layouts = Vec::with_capacity(panes.len());

    for grid_row in 0..grid_rows {
        let start = grid_row * grid_columns;
        let count = (tiled.len().saturating_sub(start)).min(grid_columns);
        if count == 0 {
            continue;
        }
        let row = &tiled[start..start + count];
        let weights = row
            .iter()
            .map(|pane| u64::from(pane.layout_weight.max(1)))
            .collect::<Vec<_>>();
        let y = distributed_edge(grid_row, grid_rows, rows);
        let next_y = distributed_edge(grid_row + 1, grid_rows, rows);
        let rows_in_pane = next_y.saturating_sub(y).max(1);
        for (grid_column, pane) in row.iter().enumerate() {
            let x = distributed_weighted_edge(grid_column, &weights, columns);
            let next_x = distributed_weighted_edge(grid_column + 1, &weights, columns);
            let columns_in_pane = next_x.saturating_sub(x).max(1);
            layouts.push(PaneLayout {
                pane_id: pane.pane_id.clone(),
                kind: pane.kind,
                layout_order: pane.layout_order,
                layout_weight: pane.layout_weight.max(1),
                z_index: pane.z_index,
                focused: focused == Some(&pane.pane_id),
                rect: LayoutRect {
                    x,
                    y,
                    rows: rows_in_pane,
                    columns: columns_in_pane,
                },
            });
        }
    }

    let mut floating = panes
        .iter()
        .filter(|pane| pane.kind == PaneKind::Floating)
        .collect::<Vec<_>>();
    floating.sort_by_key(|pane| (pane.z_index, pane.layout_order, pane.pane_id.to_string()));
    for pane in floating {
        layouts.push(PaneLayout {
            pane_id: pane.pane_id.clone(),
            kind: pane.kind,
            layout_order: pane.layout_order,
            layout_weight: pane.layout_weight.max(1),
            z_index: pane.z_index,
            focused: focused == Some(&pane.pane_id),
            rect: pane.floating_rect.map_or_else(
                || floating_rect(rows, columns),
                |rect| clamp_floating_rect(rect, rows, columns),
            ),
        });
    }

    if !layouts.iter().any(|layout| layout.focused)
        && let Some(first) = layouts.first_mut()
    {
        first.focused = true;
    }
    Ok(layouts)
}

fn clamp_floating_rect(rect: LayoutRect, rows: u16, columns: u16) -> LayoutRect {
    let rect_rows = rect.rows.clamp(1, rows);
    let rect_columns = rect.columns.clamp(1, columns);
    LayoutRect {
        x: rect.x.min(columns - rect_columns),
        y: rect.y.min(rows - rect_rows),
        rows: rect_rows,
        columns: rect_columns,
    }
}

fn floating_rect(rows: u16, columns: u16) -> LayoutRect {
    let popup_rows = (u32::from(rows) * 3 / 4).max(1) as u16;
    let popup_columns = (u32::from(columns) * 4 / 5).max(1) as u16;
    LayoutRect {
        x: (columns - popup_columns) / 2,
        y: (rows - popup_rows) / 2,
        rows: popup_rows,
        columns: popup_columns,
    }
}

fn distributed_edge(index: usize, count: usize, extent: u16) -> u16 {
    let weighted = u32::from(extent) * index as u32;
    u16::try_from(weighted / count as u32).unwrap_or(extent)
}

fn distributed_weighted_edge(index: usize, weights: &[u64], extent: u16) -> u16 {
    let total = weights.iter().sum::<u64>();
    let consumed = weights.iter().take(index).sum::<u64>();
    let weighted = u64::from(extent) * consumed;
    u16::try_from(weighted / total).unwrap_or(extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pane_uses_the_complete_viewport() {
        let panes = [PaneId::new()];
        let layout = tile_panes(&panes, 24, 80, Some(&panes[0])).unwrap();
        assert_eq!(
            layout,
            vec![PaneLayout {
                pane_id: panes[0].clone(),
                kind: PaneKind::Normal,
                layout_order: 0,
                layout_weight: 1,
                z_index: 0,
                focused: true,
                rect: LayoutRect {
                    x: 0,
                    y: 0,
                    rows: 24,
                    columns: 80
                }
            }]
        );
    }

    #[test]
    fn two_panes_split_into_balanced_columns() {
        let panes = [PaneId::new(), PaneId::new()];
        let layout = tile_panes(&panes, 20, 81, Some(&panes[1])).unwrap();
        assert_eq!(
            layout[0].rect,
            LayoutRect {
                x: 0,
                y: 0,
                rows: 20,
                columns: 40
            }
        );
        assert_eq!(
            layout[1].rect,
            LayoutRect {
                x: 40,
                y: 0,
                rows: 20,
                columns: 41
            }
        );
        assert!(!layout[0].focused);
        assert!(layout[1].focused);
    }

    #[test]
    fn five_panes_use_a_three_by_two_grid_without_overlap() {
        let panes = (0..5).map(|_| PaneId::new()).collect::<Vec<_>>();
        let layout = tile_panes(&panes, 31, 101, None).unwrap();
        let area = layout
            .iter()
            .map(|pane| u32::from(pane.rect.rows) * u32::from(pane.rect.columns))
            .sum::<u32>();
        assert_eq!(area, 31 * 101);
        assert_eq!(
            layout
                .iter()
                .filter(|pane| pane.rect.y == 0)
                .map(|pane| pane.rect.columns)
                .sum::<u16>(),
            101
        );
        assert!(layout[0].focused);
    }

    #[test]
    fn persisted_order_and_weight_control_column_geometry() {
        let first = PaneId::new();
        let second = PaneId::new();
        let layout = tile_pane_specs(
            &[
                PaneSpec {
                    pane_id: second.clone(),
                    kind: PaneKind::Normal,
                    layout_order: 1,
                    layout_weight: 3,
                    z_index: 0,
                    floating_rect: None,
                },
                PaneSpec {
                    pane_id: first.clone(),
                    kind: PaneKind::Normal,
                    layout_order: 0,
                    layout_weight: 1,
                    z_index: 0,
                    floating_rect: None,
                },
            ],
            10,
            80,
            Some(&second),
        )
        .unwrap();
        assert_eq!(layout[0].pane_id, first);
        assert_eq!(layout[0].rect.columns, 20);
        assert_eq!(layout[1].pane_id, second);
        assert_eq!(layout[1].rect.x, 20);
        assert_eq!(layout[1].rect.columns, 60);
        assert!(layout[1].focused);
    }

    #[test]
    fn floating_panes_render_after_tiled_panes_in_z_order() {
        let tiled = PaneId::new();
        let lower = PaneId::new();
        let upper = PaneId::new();
        let layout = tile_pane_specs(
            &[
                PaneSpec {
                    pane_id: upper.clone(),
                    kind: PaneKind::Floating,
                    layout_order: 3,
                    layout_weight: 1,
                    z_index: 2,
                    floating_rect: None,
                },
                PaneSpec {
                    pane_id: tiled.clone(),
                    kind: PaneKind::Normal,
                    layout_order: 0,
                    layout_weight: 1,
                    z_index: 0,
                    floating_rect: None,
                },
                PaneSpec {
                    pane_id: lower,
                    kind: PaneKind::Floating,
                    layout_order: 2,
                    layout_weight: 1,
                    z_index: 1,
                    floating_rect: None,
                },
            ],
            20,
            80,
            Some(&upper),
        )
        .unwrap();
        assert_eq!(layout[0].pane_id, tiled);
        assert_eq!(layout[0].rect.columns, 80);
        assert_eq!(layout[1].z_index, 1);
        assert_eq!(layout[2].pane_id, upper);
        assert_eq!(layout[2].kind, PaneKind::Floating);
        assert_eq!(
            layout[2].rect,
            LayoutRect {
                x: 8,
                y: 2,
                rows: 15,
                columns: 64
            }
        );
        assert!(layout[2].focused);
    }

    #[test]
    fn persisted_floating_bounds_are_clamped_to_the_viewport() {
        let pane = PaneId::new();
        let layout = tile_pane_specs(
            &[PaneSpec {
                pane_id: pane.clone(),
                kind: PaneKind::Floating,
                layout_order: 1,
                layout_weight: 1,
                z_index: 1,
                floating_rect: Some(LayoutRect {
                    x: 70,
                    y: 18,
                    rows: 20,
                    columns: 30,
                }),
            }],
            20,
            80,
            Some(&pane),
        )
        .unwrap();
        assert_eq!(
            layout[0].rect,
            LayoutRect {
                x: 50,
                y: 0,
                rows: 20,
                columns: 30
            }
        );
    }

    #[test]
    fn invalid_viewport_and_empty_session_are_explicit() {
        assert_eq!(tile_panes(&[], 24, 80, None), Err(LayoutError::NoPanes));
        let panes = [PaneId::new()];
        assert_eq!(
            tile_panes(&panes, 0, 80, None),
            Err(LayoutError::InvalidViewport {
                rows: 0,
                columns: 80
            })
        );
    }
}
