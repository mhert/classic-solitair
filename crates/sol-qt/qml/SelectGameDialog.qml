// "Select Game…": deal a specific game by its seed number.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Dialog {
    id: dialog
    required property var playfield

    title: qsTr("Select Game")
    modal: true
    anchors.centerIn: parent
    standardButtons: Dialog.Ok | Dialog.Cancel

    onAboutToShow: {
        seedField.text = playfield.seedText
        seedField.selectAll()
        seedField.forceActiveFocus()
    }
    onAccepted: {
        if (!playfield.selectGame(seedField.text))
            playfield.statusMessage = qsTr("Enter a game number from 0 to 32767")
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 8
        Label { text: qsTr("Game number (0 – 32767):") }
        TextField {
            id: seedField
            Layout.fillWidth: true
            validator: IntValidator { bottom: 0; top: 32767 }
            onAccepted: dialog.accept()
        }
        Label {
            text: qsTr("The same number always deals the same game.")
            font.italic: true
        }
    }
}
