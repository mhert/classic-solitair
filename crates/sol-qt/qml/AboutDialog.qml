// Help → About.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Dialog {
    id: dialog
    title: qsTr("About classic-solitair")
    modal: true
    anchors.centerIn: parent
    standardButtons: Dialog.Close

    ColumnLayout {
        anchors.fill: parent
        spacing: 8
        Label {
            text: qsTr("classic-solitair")
            font.bold: true
        }
        Label {
            Layout.maximumWidth: 400
            wrapMode: Text.WordWrap
            text: qsTr("A faithful reproduction of Windows 98 Klondike Solitaire, "
                       + "extended with save/load, undo/redo, themes, and seed-based "
                       + "game selection.")
        }
        Label {
            Layout.maximumWidth: 400
            wrapMode: Text.WordWrap
            text: qsTr("Free software under the GNU GPL 3.0 or later. "
                       + "No original Microsoft artwork is included; use "
                       + "soltool extract to build a theme from your own files.")
        }
    }
}
