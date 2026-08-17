// Game → Options…: draw mode, scoring, flags, and the theme/card-back/
// card-scaling pickers. Theme, back and scaling selections apply to the
// board immediately (live preview); Cancel restores them, OK commits
// everything.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Dialog {
    id: dialog
    required property var playfield

    title: qsTr("Options")
    modal: true
    anchors.centerIn: parent
    standardButtons: Dialog.Ok | Dialog.Cancel

    // Populate from the presenter's current options. The live-preview
    // restore point is captured first, before any control is touched:
    // filling the dialog only reads, but the point Cancel returns to
    // must be the state the dialog was opened on, and nothing that runs
    // afterwards can be allowed to define it.
    function openWithCurrent() {
        playfield.beginPreview()
        drawThree.checked = playfield.optionDrawThree()
        drawOne.checked = !drawThree.checked
        const scoring = playfield.optionScoring()
        scoreStandard.checked = scoring === "standard"
        scoreVegas.checked = scoring === "vegas"
        scoreNone.checked = scoring === "none"
        timedBox.checked = playfield.optionTimed()
        outlineBox.checked = playfield.optionOutline()
        keepVegasBox.checked = playfield.optionKeepVegas()
        soundsBox.checked = playfield.optionSounds()
        themeCombo.model = playfield.themeIds()
        themeCombo.currentIndex = themeCombo.indexOfValue(playfield.themeId())
        refreshBacks()
        open()
    }

    // Re-syncs the card-back grid and the card-scaling picker to what is
    // actually active — on open, and after a theme switch, since a theme
    // brings its own backs and its own scaling choice.
    function refreshBacks() {
        // Both assignments below move the grid's selection without the
        // player touching anything: handing a GridView a model makes it
        // take a current item of its own accord, and the line after
        // that puts the selection where the active back is. Fencing them
        // off keeps the selection handler for what it is named after —
        // the player picking a back — so merely opening this dialog can
        // never change which card back is in play.
        backGrid.syncing = true
        backGrid.model = playfield.backNames()
        backGrid.currentIndex = playfield.backIndex()
        backGrid.syncing = false
        scalingCombo.currentIndex = playfield.scalingIndex()
        scalingCombo.enabled = playfield.scalingIsAvailable()
        refreshBackPreviews()
    }

    // Rebuilds the grid's thumbnails for whatever artwork is active now.
    // Needed after anything that changes what a card back looks like —
    // a theme switch or a card-scaling switch — because the bridge drops
    // its preview cache on both, and an uncached grid neither redraws
    // nor animates until something asks for it again.
    function refreshBackPreviews() {
        const error = playfield.refreshBackPreviews()
        if (error !== "")
            playfield.statusMessage = error
    }

    onAccepted: playfield.commitOptions(
        drawThree.checked,
        scoreVegas.checked ? "vegas" : (scoreNone.checked ? "none" : "standard"),
        timedBox.checked,
        outlineBox.checked,
        keepVegasBox.checked,
        soundsBox.checked)
    onRejected: playfield.cancelPreview()

    GridLayout {
        anchors.fill: parent
        columns: 2
        columnSpacing: 24
        rowSpacing: 4

        GroupBox {
            title: qsTr("Draw")
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignTop
            ColumnLayout {
                RadioButton { id: drawOne; text: qsTr("Draw &one") }
                RadioButton { id: drawThree; text: qsTr("Draw &three") }
            }
        }

        GroupBox {
            title: qsTr("Scoring")
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignTop
            ColumnLayout {
                RadioButton { id: scoreStandard; text: qsTr("&Standard") }
                RadioButton { id: scoreVegas; text: qsTr("&Vegas") }
                RadioButton { id: scoreNone; text: qsTr("&None") }
            }
        }

        ColumnLayout {
            Layout.columnSpan: 2
            CheckBox { id: timedBox; text: qsTr("Ti&med game") }
            CheckBox { id: outlineBox; text: qsTr("Outline &dragging") }
            CheckBox {
                id: keepVegasBox
                text: qsTr("&Keep Vegas score between games")
                enabled: scoreVegas.checked
            }
            CheckBox { id: soundsBox; text: qsTr("So&unds") }
        }

        Label { text: qsTr("Theme:") }
        ComboBox {
            id: themeCombo
            Layout.fillWidth: true
            onActivated: {
                const error = playfield.previewTheme(currentText)
                if (error !== "") {
                    playfield.statusMessage = error
                    currentIndex = indexOfValue(playfield.themeId())
                }
                dialog.refreshBacks()
            }
        }

        Label {
            text: qsTr("Card back:")
            Layout.alignment: Qt.AlignTop
        }
        ScrollView {
            Layout.fillWidth: true
            Layout.preferredHeight: backGrid.cellHeight * 3
            clip: true

            GridView {
                id: backGrid
                cellWidth: Math.max(playfield.backCellWidth, 48) + 12
                cellHeight: Math.max(playfield.backCellHeight, 48) + 12
                // Reachable by Tab and, once it has focus, navigable with
                // the arrow keys — what the combo box this grid replaced
                // gave a keyboard-only player for free. GridView's own
                // key navigation is enabled by default; all it lacks is
                // ever being given the focus to receive those keys.
                activeFocusOnTab: true
                // Raised around the imperative selection moves in
                // refreshBacks() — see there. While it is up, a
                // currentIndex change is this dialog's own doing and must
                // not be reported as the player choosing a back.
                property bool syncing: false
                // Every other route to a new currentIndex is the player:
                // a click below, or an arrow key the view turns into a
                // move. Both preview the back live, on this one path.
                onCurrentIndexChanged: {
                    if (!backGrid.syncing)
                        playfield.previewBack(currentIndex)
                }

                delegate: Rectangle {
                    id: backDelegate
                    required property string modelData
                    required property int index
                    width: backGrid.cellWidth
                    height: backGrid.cellHeight
                    color: "transparent"
                    border.width: 2
                    border.color: GridView.isCurrentItem ? palette.highlight : "transparent"

                    property string frameUri: playfield.backFrameUri(index)
                    Connections {
                        target: playfield
                        function onBackFrameEpochChanged() {
                            backDelegate.frameUri = playfield.backFrameUri(backDelegate.index)
                        }
                    }

                    Image {
                        // The PNG is rendered at the sheet's own scale
                        // (the display's DPR rounded up), so its pixel
                        // size is already the device pixels one logical
                        // cell needs; without this explicit logical size
                        // Image would treat those pixels as 1:1 logical
                        // units and, on a HiDPI display, Qt's own scene
                        // graph would then blow the already-scaled image
                        // up by the DPR a second time.
                        anchors.centerIn: parent
                        width: playfield.backCellWidth
                        height: playfield.backCellHeight
                        fillMode: Image.PreserveAspectFit
                        source: backDelegate.frameUri
                        visible: backDelegate.frameUri !== ""
                        smooth: false
                    }
                    Label {
                        anchors.fill: parent
                        anchors.margins: 6
                        visible: backDelegate.frameUri === ""
                        text: backDelegate.modelData
                        wrapMode: Text.WordWrap
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }

                    MouseArea {
                        anchors.fill: parent
                        // Taking the focus is what lets the arrow keys
                        // continue from wherever the player just clicked.
                        onClicked: {
                            backGrid.forceActiveFocus()
                            backGrid.currentIndex = backDelegate.index
                        }
                    }
                }
            }
        }

        Label { text: qsTr("Card scaling:") }
        ComboBox {
            id: scalingCombo
            Layout.fillWidth: true
            model: [qsTr("Original"), qsTr("xBRZ")]
            onActivated: {
                const error = playfield.previewScaling(currentIndex)
                if (error !== "") {
                    playfield.statusMessage = error
                    currentIndex = playfield.scalingIndex()
                }
                // The card artwork itself may have changed (a PNG
                // theme's xBRZ smoothing is a card-scaling choice), so
                // the thumbnails need the same rebuild a theme change
                // gets — whether the switch took or the picker just
                // snapped back.
                dialog.refreshBackPreviews()
            }
        }

        Label {
            Layout.columnSpan: 2
            Layout.maximumWidth: 420
            wrapMode: Text.WordWrap
            font.italic: true
            text: qsTr("Theme and card scaling preview on the board behind "
                       + "this dialog; card backs preview right here in the "
                       + "grid. Cancel puts everything back.")
        }
    }
}
