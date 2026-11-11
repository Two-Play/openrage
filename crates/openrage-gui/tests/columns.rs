//! UI test with Slint's testing backend (no display needed): dragging a
//! divider in the file list header resizes the column.

slint::include_modules!();

use i_slint_backend_testing::ElementHandle;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};

#[test]
fn dragging_the_type_divider_widens_the_column() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().expect("the main window builds");
    ui.window().set_size(slint::LogicalSize::new(1400.0, 900.0));
    ui.show().expect("the window shows");

    let cells: Vec<ElementHandle> = ElementHandle::find_by_element_type_name(&ui, "HeaderCell").collect();
    let labels: Vec<String> = cells
        .iter()
        .map(|c| format!("{:?} {:?} {:?}", c.accessible_label(), c.absolute_position(), c.size()))
        .collect();
    let cell = cells
        .into_iter()
        .find(|e| e.accessible_label().is_some_and(|l| l == "Type"))
        .unwrap_or_else(|| panic!("the Type column header among {labels:?}"));
    let (pos, size) = (cell.absolute_position(), cell.size());
    let before = ui.get_col_kind_width();

    // The divider sits at the cell's left edge; dragging it left widens the column.
    let y = pos.y + size.height / 2.0;
    let start = LogicalPosition::new(pos.x + 2.0, y);
    let window = ui.window();
    window.dispatch_event(WindowEvent::PointerMoved { position: start });
    window.dispatch_event(WindowEvent::PointerPressed { position: start, button: PointerEventButton::Left });
    for step in 1..=4 {
        let p = LogicalPosition::new(start.x - 10.0 * step as f32, y);
        window.dispatch_event(WindowEvent::PointerMoved { position: p });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(start.x - 40.0, y),
        button: PointerEventButton::Left,
    });

    let after = ui.get_col_kind_width();
    assert!((after - before - 40.0).abs() < 1.0, "width {before} → {after}, expected +40");
}
