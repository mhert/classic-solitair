// The application window: menu bar, playfield, status bar. Every action
// calls into the Playfield bridge object — no game logic lives in QML.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import ClassicSolitair

ApplicationWindow {
    id: appWindow
    // The 2×-scaled design client plus room for the chrome; freely
    // resizable — the felt absorbs slack, like the original.
    width: 1200
    height: 850
    visible: true
    title: qsTr("classic-solitair")

    menuBar: MenuBar {
        Menu {
            title: qsTr("&Game")
            Action {
                text: qsTr("&Deal")
                shortcut: "F2"
                onTriggered: playfield.deal()
            }
            Action {
                text: qsTr("&Select Game…")
                onTriggered: selectGameDialog.open()
            }
            MenuSeparator {}
            Action {
                text: qsTr("&Undo")
                shortcut: "Ctrl+Z"
                enabled: playfield.canUndo
                onTriggered: playfield.undo()
            }
            Action {
                text: qsTr("&Redo")
                shortcut: "Ctrl+Y"
                enabled: playfield.canRedo
                onTriggered: playfield.redo()
            }
            MenuSeparator {}
            Action {
                text: qsTr("Sa&ve")
                onTriggered: playfield.save()
            }
            Action {
                text: qsTr("&Load")
                onTriggered: playfield.load()
            }
            MenuSeparator {}
            Action {
                text: qsTr("&Options…")
                onTriggered: optionsDialog.openWithCurrent()
            }
            MenuSeparator {}
            Action {
                text: qsTr("E&xit")
                onTriggered: appWindow.close()
            }
        }
        Menu {
            title: qsTr("&Help")
            Action {
                text: qsTr("&About")
                onTriggered: aboutDialog.open()
            }
        }
    }

    Playfield {
        id: playfield
        anchors.fill: parent
        // The frame image is already at physical resolution; never let
        // the scene graph smooth it (parity: zero smoothing).
        smooth: false
        focus: true

        function sendSize() {
            playfield.viewResized(width, height, Screen.devicePixelRatio)
        }
        onWidthChanged: sendSize()
        onHeightChanged: sendSize()
        Screen.onDevicePixelRatioChanged: sendSize()
        Component.onCompleted: sendSize()

        // Any key lands running animations (deal, cascade), like the
        // original; menu shortcuts still fire through their Actions.
        Keys.onPressed: playfield.anyKey()

        onWonChanged: if (playfield.won) gameWonDialog.open()

        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.LeftButton
            onPressed: mouse => playfield.press(mouse.x, mouse.y)
            onPositionChanged: mouse => playfield.moveTo(mouse.x, mouse.y)
            onReleased: mouse => playfield.release(mouse.x, mouse.y)
        }

        // Drives animations, the clock, and rendering; Fifo-style
        // pacing is not available to a paced readback loop, so a plain
        // ~60 Hz timer stands in.
        Timer {
            interval: 16
            repeat: true
            running: appWindow.visible
            onTriggered: playfield.tick()
        }
    }

    footer: ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8
            anchors.rightMargin: 8
            Label { text: qsTr("Game") }
            TextInput {
                id: seedField
                text: playfield.seedText
                readOnly: true
                selectByMouse: true
                color: palette.text
                Accessible.name: qsTr("Current game seed")
            }
            ToolButton {
                text: qsTr("Copy")
                Accessible.name: qsTr("Copy seed")
                onClicked: {
                    seedField.selectAll()
                    seedField.copy()
                    seedField.deselect()
                }
            }
            Label {
                id: statusLabel
                Layout.fillWidth: true
                elide: Text.ElideRight
                horizontalAlignment: Text.AlignHCenter
                text: playfield.statusMessage
            }
            Label { text: playfield.scoreText }
            Label { text: playfield.timeText }
        }
    }

    // Status messages fade after a moment.
    Timer {
        id: statusClear
        interval: 4000
        onTriggered: playfield.statusMessage = ""
    }
    Connections {
        target: playfield
        function onStatusMessageChanged() {
            if (playfield.statusMessage !== "")
                statusClear.restart()
        }
    }

    onClosing: {
        // The settle debounce may still be pending here: a move or
        // resize made in the last half second before closing lives only
        // in the window, not yet in the recorded geometry. Capturing it
        // now — while the window still exists — lets the exit persist
        // below write the truly last placement, in a single write.
        appWindow.recordLiveGeometry()
        playfield.autosaveOnExit()
    }

    // Restores the previous session's window placement — size (clamped
    // to a 400×300 floor and the available desktop ceiling), position,
    // and maximized state — then runs the existing --smoke self-test
    // hook, folded into the same handler: QML allows only one
    // Component.onCompleted per object.
    Component.onCompleted: {
        if (playfield.initialWindowWidth() > 0 && playfield.initialWindowHeight() > 0) {
            appWindow.width = Math.min(Math.max(playfield.initialWindowWidth(), 400),
                                        Math.max(Screen.desktopAvailableWidth, 400))
            appWindow.height = Math.min(Math.max(playfield.initialWindowHeight(), 300),
                                         Math.max(Screen.desktopAvailableHeight, 300))
        }
        // A saved position is applied only when it still lands on a
        // connected screen: a monitor that went away would otherwise
        // strand the window off the visible desktop. Skipping it leaves
        // the placement to the windowing system. Applying it is itself a
        // no-op under Wayland, where the compositor owns top-level
        // placement and an application-requested x/y is ignored (nothing
        // is persisted there either).
        if (playfield.hasInitialWindowPosition()
                && appWindow.positionIsOnScreen(playfield.initialWindowX(),
                                                playfield.initialWindowY())) {
            appWindow.x = playfield.initialWindowX()
            appWindow.y = playfield.initialWindowY()
        }
        if (playfield.initialWindowMaximized()) {
            appWindow.visibility = Window.Maximized
        }

        // --smoke self-test: resize the window down then back up (crossing
        // atlas factor boundaries so the async adopt/build/apply rebuild
        // runs headlessly) and instantiate every dialog once (they
        // otherwise load lazily, so a plain launch never exercises them),
        // running the render loop meanwhile, then exit. Driven headlessly by
        // the qml_smoke integration test under QT_QPA_PLATFORM=offscreen.
        if (playfield.smokeMode()) {
            smokeResizeShrink.start()
            smokeOpen.start()
        }
    }

    // Minimized and hidden windows report geometry that is not the
    // restorable one, so only these two states are worth capturing.
    readonly property bool geometryCapturable: appWindow.visibility === Window.Windowed
                                               || appWindow.visibility === Window.Maximized

    // Whether a restored top-left still lands on a connected screen. A
    // band the size of a grabbable title-bar corner must intersect some
    // screen of the virtual desktop — the bare corner pixel being
    // on-screen is not enough to make the window usable.
    function positionIsOnScreen(posX, posY) {
        const grabWidth = 120
        const grabHeight = 40
        const screens = Qt.application.screens
        for (let i = 0; i < screens.length; ++i) {
            const screen = screens[i]
            if (posX + grabWidth > screen.virtualX && posX < screen.virtualX + screen.width
                    && posY + grabHeight > screen.virtualY && posY < screen.virtualY + screen.height)
                return true
        }
        return false
    }

    // Captures the window's current geometry without writing it, for the
    // exit paths: the persist that follows them carries it to disk.
    function recordLiveGeometry() {
        if (appWindow.geometryCapturable) {
            playfield.recordWindowGeometry(appWindow.width, appWindow.height,
                                            appWindow.x, appWindow.y,
                                            appWindow.visibility === Window.Maximized)
        }
    }

    // Debounced geometry capture: a burst of changes (an interactive
    // drag, a maximize/restore) keeps restarting the timer, so only the
    // settled geometry — the state ~500 ms after the last change — is
    // recorded and persisted.
    Timer {
        id: geometrySettle
        interval: 500
        onTriggered: {
            if (appWindow.geometryCapturable) {
                playfield.windowGeometrySettled(appWindow.width, appWindow.height,
                                                 appWindow.x, appWindow.y,
                                                 appWindow.visibility === Window.Maximized)
            }
        }
    }
    onWidthChanged: geometrySettle.restart()
    onHeightChanged: geometrySettle.restart()
    onXChanged: geometrySettle.restart()
    onYChanged: geometrySettle.restart()
    onVisibilityChanged: geometrySettle.restart()

    // Shrinks below launch size, landing the playfield at atlas factor
    // 2, then chains to the growth resize below.
    Timer {
        id: smokeResizeShrink
        interval: 200
        onTriggered: {
            appWindow.width = 620
            appWindow.height = 470
            smokeResizeGrow.start()
        }
    }
    // Grows past launch size, crossing the atlas factor 2 → 3 boundary
    // so the async adopt/build/apply rebuild runs headlessly, well
    // before the dialog-open step below.
    Timer {
        id: smokeResizeGrow
        interval: 200
        onTriggered: {
            appWindow.width = 1250
            appWindow.height = 950
        }
    }
    // Every self-test failure below goes out under one prefix, so the
    // smoke run's stderr scan has a single thing to look for. QML's
    // console.error prints the bare message, with none of the
    // "<file>.qml:<line>" a real QML diagnostic carries.
    function smokeFail(reason) {
        console.error("smoke check failed: " + reason)
    }

    Timer {
        id: smokeOpen
        interval: 700
        onTriggered: {
            // Opening Options must show the player's choices, never
            // rewrite them. The card back is the one shown through a view
            // that moves its own selection when it is handed a model, so
            // it is the one this pins — read before, compared after.
            const backBefore = playfield.backIndex()
            optionsDialog.openWithCurrent()
            if (playfield.backIndex() !== backBefore)
                appWindow.smokeFail("opening Options moved the card back from "
                                    + backBefore + " to " + playfield.backIndex())
            // openWithCurrent() already rebuilt the preview grid via
            // refreshBacks(); asking again here is a cache hit (a matching
            // key returns immediately) that hands back the same verdict.
            const previewError = playfield.refreshBackPreviews()
            if (previewError !== "")
                appWindow.smokeFail("card back preview rebuild: " + previewError)
            // Catches a total failure of the cell-size wiring (stuck at the
            // 0 default, or some nonsensical value) — not a proof that the
            // GridView/Image bindings actually re-evaluate on a later
            // change, which would need a second theme with a different
            // card size that this single-theme smoke run never loads.
            if (playfield.backCellWidth <= 0 || playfield.backCellWidth > 2048
                    || playfield.backCellHeight <= 0 || playfield.backCellHeight > 2048)
                appWindow.smokeFail("card back cell size is implausible: "
                                    + playfield.backCellWidth + "x" + playfield.backCellHeight)
            selectGameDialog.open()
            gameWonDialog.open()
            aboutDialog.open()
            smokeQuit.start()
        }
    }
    Timer {
        id: smokeQuit
        interval: 700
        onTriggered: {
            aboutDialog.close()
            gameWonDialog.close()
            selectGameDialog.close()
            optionsDialog.reject()
            // Qt.exit bypasses onClosing, so the real exit contract
            // (capture the live geometry, then autosave + settings
            // persist) is invoked explicitly here — otherwise the smoke
            // run would never exercise it.
            appWindow.recordLiveGeometry()
            playfield.autosaveOnExit()
            Qt.exit(0)
        }
    }

    SelectGameDialog {
        id: selectGameDialog
        playfield: playfield
    }
    OptionsDialog {
        id: optionsDialog
        playfield: playfield
    }
    GameWonDialog {
        id: gameWonDialog
        playfield: playfield
    }
    AboutDialog {
        id: aboutDialog
    }
}
