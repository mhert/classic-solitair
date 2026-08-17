// Shown once the win cascade has settled.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Dialog {
    id: dialog
    required property var playfield

    title: qsTr("Game Won")
    modal: true
    anchors.centerIn: parent

    onAccepted: playfield.deal()
    onClosed: playfield.won = false

    ColumnLayout {
        anchors.fill: parent
        spacing: 8
        Label {
            text: qsTr("Congratulations, you won!")
            font.bold: true
        }
        Label {
            visible: playfield.scoreText !== ""
            text: playfield.scoreText
        }
        Label { text: qsTr("Deal another game?") }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Deal Again")
            DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
        }
        Button {
            text: qsTr("Close")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
        }
    }
}
