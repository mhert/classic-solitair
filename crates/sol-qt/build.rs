//! Generates and compiles the cxx-qt bridge: C++ for `src/bridge.rs`,
//! the QML module resources, and the Qt link flags. Qt is located via
//! `qmake6`/`qmake` on `PATH`.

use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("ClassicSolitair")
            .qml_files([
                "qml/Main.qml",
                "qml/OptionsDialog.qml",
                "qml/SelectGameDialog.qml",
                "qml/GameWonDialog.qml",
                "qml/AboutDialog.qml",
            ])
            // QQuickPaintedItem lives in QtQuick; declare it for qmllint.
            .depend("QtQuick"),
    )
    // Qt Quick provides QQuickPaintedItem, the playfield item's base
    // class (Core/Gui/Qml arrive via cxx-qt-lib's qt_full feature).
    .qt_module("Quick")
    .files(["src/bridge.rs"])
    .build();
}
