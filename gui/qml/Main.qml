import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Effects
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.ApplicationWindow {
    id: root

    required property var receiver

    readonly property bool running: receiver.unitState === "active"
    readonly property bool streaming: running && receiver.online
                                      && (receiver.active || receiver.playbackStatus !== "Stopped")
    readonly property bool playing: receiver.playbackStatus === "Playing"
    readonly property bool hasTrack: streaming && receiver.title !== ""
    readonly property string speakerName: receiver.receiverName || "this computer"

    // Output choices: "" follows the system default; anything else is a sink's node.name.
    readonly property var outputEntries: {
        const entries = [{
            name: "",
            label: receiver.defaultOutput ? "Follow System (" + receiver.defaultOutput + ")" : "Follow System"
        }];
        for (const output of receiver.outputs)
            entries.push({ name: output.name, label: output.description });
        const target = receiver.outputTarget;
        if (target !== "" && !receiver.outputs.some(output => output.name === target))
            entries.push({ name: target, label: (receiver.outputTargetLabel || target) + " (not connected)" });
        return entries;
    }
    readonly property int outputIndex: Math.max(0, outputEntries.findIndex(e => e.name === receiver.outputTarget))

    function formatTime(seconds) {
        const total = Math.max(0, Math.floor(seconds));
        const h = Math.floor(total / 3600);
        const m = Math.floor((total % 3600) / 60);
        const s = total % 60;
        const ss = (s < 10 ? "0" : "") + s;
        return h > 0 ? h + ":" + (m < 10 ? "0" : "") + m + ":" + ss : m + ":" + ss;
    }

    title: hasTrack ? receiver.title + (receiver.artist ? " — " + receiver.artist : "")
                    : "AirPlay Receiver"
    width: Kirigami.Units.gridUnit * 24
    height: Kirigami.Units.gridUnit * 38
    minimumWidth: Kirigami.Units.gridUnit * 18
    minimumHeight: Kirigami.Units.gridUnit * 28

    Connections {
        target: root.receiver
        function onErrorOccurred(message) {
            root.showPassiveNotification(message, "long");
        }
    }

    Shortcut {
        sequences: ["Media Play", "Media Pause", "Toggle Media Play/Pause", "Space"]
        enabled: root.streaming
        onActivated: root.receiver.playPause()
    }
    Shortcut {
        sequence: "Media Next"
        enabled: root.streaming
        onActivated: root.receiver.next()
    }
    Shortcut {
        sequence: "Media Previous"
        enabled: root.streaming
        onActivated: root.receiver.previous()
    }

    Kirigami.Action {
        id: startAction
        text: "Start Receiver"
        icon.name: "media-playback-start"
        onTriggered: root.receiver.startReceiver()
    }

    pageStack.initialPage: Kirigami.Page {
        id: page

        title: "AirPlay Receiver"
        padding: Kirigami.Units.largeSpacing * 2

        actions: [
            Kirigami.Action {
                text: "Disconnect"
                tooltip: "Drop the current AirPlay sender"
                icon.name: "network-disconnect"
                visible: root.streaming
                onTriggered: root.receiver.disconnectSender()
            },
            Kirigami.Action {
                text: root.running ? "Stop Receiver" : "Start Receiver"
                icon.name: root.running ? "system-shutdown" : "media-playback-start"
                onTriggered: root.running ? root.receiver.stopReceiver() : root.receiver.startReceiver()
            },
            Kirigami.Action {
                text: "Restart Receiver"
                icon.name: "view-refresh"
                displayHint: Kirigami.DisplayHint.AlwaysHide
                onTriggered: root.receiver.restartReceiver()
            },
            Kirigami.Action {
                text: "Edit Settings…"
                icon.name: "document-edit"
                displayHint: Kirigami.DisplayHint.AlwaysHide
                onTriggered: root.receiver.openConfig()
            }
        ]

        // Blurred album art behind everything while something is playing.
        background: Rectangle {
            color: Kirigami.Theme.backgroundColor
            clip: true

            Image {
                id: backdrop
                anchors.fill: parent
                source: root.hasTrack ? root.receiver.artUrl : ""
                sourceSize: Qt.size(128, 128)
                fillMode: Image.PreserveAspectCrop
                asynchronous: true
                visible: false
            }
            MultiEffect {
                anchors.fill: parent
                source: backdrop
                autoPaddingEnabled: false
                blurEnabled: true
                blur: 1.0
                blurMax: 64
                saturation: 0.3
                opacity: backdrop.status === Image.Ready ? 0.45 : 0
                Behavior on opacity {
                    NumberAnimation { duration: Kirigami.Units.longDuration }
                }
            }
        }

        // ---- receiver stopped / starting / idle ---------------------------

        Kirigami.PlaceholderMessage {
            anchors.centerIn: parent
            width: parent.width - Kirigami.Units.gridUnit * 2
            visible: !root.running
            icon.name: root.receiver.unitState === "failed" ? "dialog-error" : "network-offline"
            text: root.receiver.unitState === "failed" ? "The receiver has stopped working"
                                                        : "The receiver is turned off"
            explanation: root.receiver.unitState === "failed"
                         ? "Check its log with “journalctl --user -u shairport-sync”, then start it again."
                         : "Start it to make this computer available as an AirPlay speaker."
            helpfulAction: startAction
        }

        Kirigami.PlaceholderMessage {
            anchors.centerIn: parent
            width: parent.width - Kirigami.Units.gridUnit * 2
            visible: root.running && !root.receiver.online
            icon.name: "network-connect"
            text: "Connecting to the receiver…"
            explanation: "If this doesn't go away, make sure dbus_service_bus and mpris_service_bus "
                         + "are set to \"session\" in the receiver's settings."
        }

        Kirigami.PlaceholderMessage {
            anchors.centerIn: parent
            width: parent.width - Kirigami.Units.gridUnit * 2
            visible: root.running && root.receiver.online && !root.streaming
            icon.source: Qt.resolvedUrl("../shairport-sync.svg")
            text: "Ready for AirPlay"
            explanation: "Choose “" + root.speakerName + "” as the speaker on your iPhone, iPad or Mac."
        }

        // ---- now playing --------------------------------------------------

        ColumnLayout {
            id: nowPlaying

            anchors.fill: parent
            visible: root.streaming
            spacing: Kirigami.Units.largeSpacing

            Kirigami.Chip {
                Layout.alignment: Qt.AlignHCenter
                Layout.maximumWidth: nowPlaying.width
                checkable: false
                closable: false
                icon.name: "smartphone"
                text: root.receiver.clientName ? "Streaming from " + root.receiver.clientName
                                               : "Streaming to " + root.speakerName
            }

            Item {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.minimumHeight: Kirigami.Units.gridUnit * 6

                Kirigami.ShadowedRectangle {
                    id: artFrame

                    readonly property real side: Math.min(parent.width, parent.height)

                    anchors.centerIn: parent
                    width: side
                    height: side
                    radius: Kirigami.Units.cornerRadius * 3
                    color: Kirigami.Theme.alternateBackgroundColor
                    shadow.size: Kirigami.Units.gridUnit
                    shadow.yOffset: Kirigami.Units.smallSpacing
                    shadow.color: Qt.rgba(0, 0, 0, 0.35)

                    Kirigami.Icon {
                        anchors.centerIn: parent
                        width: parent.width * 0.4
                        height: width
                        visible: art.status !== Image.Ready
                        source: "media-album-cover"
                        opacity: 0.5
                    }

                    Kirigami.ShadowedImage {
                        id: art
                        anchors.fill: parent
                        radius: artFrame.radius
                        source: root.hasTrack ? root.receiver.artUrl : ""
                        sourceSize: Qt.size(1024, 1024)
                        fillMode: Image.PreserveAspectCrop
                        asynchronous: true
                        mipmap: true
                        opacity: status === Image.Ready ? 1 : 0
                        Behavior on opacity {
                            NumberAnimation { duration: Kirigami.Units.longDuration }
                        }
                    }
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing

                Kirigami.Heading {
                    Layout.fillWidth: true
                    level: 2
                    text: root.receiver.title || (root.playing ? "Playing" : "Connected")
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    maximumLineCount: 2
                    elide: Text.ElideRight
                }
                QQC2.Label {
                    Layout.fillWidth: true
                    text: root.receiver.artist
                    visible: text !== ""
                    horizontalAlignment: Text.AlignHCenter
                    elide: Text.ElideRight
                    font.weight: Font.DemiBold
                }
                QQC2.Label {
                    Layout.fillWidth: true
                    text: root.receiver.album
                    visible: text !== ""
                    horizontalAlignment: Text.AlignHCenter
                    elide: Text.ElideRight
                    opacity: 0.7
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0
                visible: root.receiver.duration > 0

                // Read-only track: AirPlay senders don't accept seeking from the receiver.
                Rectangle {
                    Layout.fillWidth: true
                    Layout.topMargin: Kirigami.Units.smallSpacing
                    Layout.bottomMargin: Kirigami.Units.smallSpacing
                    implicitHeight: Math.round(Kirigami.Units.smallSpacing * 1.5)
                    radius: height / 2
                    color: Qt.rgba(Kirigami.Theme.textColor.r, Kirigami.Theme.textColor.g,
                                   Kirigami.Theme.textColor.b, 0.15)

                    Rectangle {
                        height: parent.height
                        radius: parent.radius
                        width: parent.width * Math.min(1, root.receiver.position / root.receiver.duration)
                        color: Kirigami.Theme.highlightColor
                        Behavior on width {
                            enabled: root.playing
                            NumberAnimation { duration: 500 }
                        }
                    }
                }
                RowLayout {
                    Layout.fillWidth: true
                    QQC2.Label {
                        text: root.formatTime(root.receiver.position)
                        font: Kirigami.Theme.smallFont
                        opacity: 0.7
                    }
                    Item { Layout.fillWidth: true }
                    QQC2.Label {
                        text: "-" + root.formatTime(root.receiver.duration - root.receiver.position)
                        font: Kirigami.Theme.smallFont
                        opacity: 0.7
                    }
                }
            }

            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                spacing: Kirigami.Units.gridUnit

                QQC2.ToolButton {
                    icon.name: "media-skip-backward"
                    icon.width: Kirigami.Units.iconSizes.medium
                    icon.height: Kirigami.Units.iconSizes.medium
                    display: QQC2.AbstractButton.IconOnly
                    text: "Previous"
                    onClicked: root.receiver.previous()
                    QQC2.ToolTip.text: text
                    QQC2.ToolTip.visible: hovered
                    QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
                QQC2.RoundButton {
                    implicitWidth: Kirigami.Units.gridUnit * 3.5
                    implicitHeight: implicitWidth
                    highlighted: true
                    icon.name: root.playing ? "media-playback-pause" : "media-playback-start"
                    icon.width: Kirigami.Units.iconSizes.medium
                    icon.height: Kirigami.Units.iconSizes.medium
                    display: QQC2.AbstractButton.IconOnly
                    text: root.playing ? "Pause" : "Play"
                    onClicked: root.receiver.playPause()
                    QQC2.ToolTip.text: text
                    QQC2.ToolTip.visible: hovered
                    QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
                QQC2.ToolButton {
                    icon.name: "media-skip-forward"
                    icon.width: Kirigami.Units.iconSizes.medium
                    icon.height: Kirigami.Units.iconSizes.medium
                    display: QQC2.AbstractButton.IconOnly
                    text: "Next"
                    onClicked: root.receiver.next()
                    QQC2.ToolTip.text: text
                    QQC2.ToolTip.visible: hovered
                    QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.maximumWidth: Kirigami.Units.gridUnit * 16
                Layout.alignment: Qt.AlignHCenter

                Kirigami.Icon {
                    source: "audio-volume-low"
                    implicitWidth: Kirigami.Units.iconSizes.small
                    implicitHeight: implicitWidth
                }
                QQC2.Slider {
                    id: volumeSlider
                    Layout.fillWidth: true
                    from: 0
                    to: 1
                    onMoved: root.receiver.setVolume(value)
                    Binding on value {
                        value: root.receiver.volume
                        when: !volumeSlider.pressed
                    }
                }
                Kirigami.Icon {
                    source: "audio-volume-high"
                    implicitWidth: Kirigami.Units.iconSizes.small
                    implicitHeight: implicitWidth
                }
            }

            QQC2.Label {
                Layout.fillWidth: true
                readonly property var formats: [root.receiver.sourceFormat, root.receiver.outputFormat]
                                               .filter(f => f !== "")
                text: formats.join("  →  ")
                visible: text !== ""
                horizontalAlignment: Text.AlignHCenter
                elide: Text.ElideMiddle
                font: Kirigami.Theme.smallFont
                opacity: 0.6
            }
        }

        footer: ColumnLayout {
            spacing: 0

            QQC2.Label {
                Layout.fillWidth: true
                padding: Kirigami.Units.smallSpacing
                visible: root.receiver.version !== "" && !root.streaming
                text: "Shairport Sync " + root.receiver.version + " · AirPlay 2"
                horizontalAlignment: Text.AlignHCenter
                font: Kirigami.Theme.smallFont
                opacity: 0.5
            }

            QQC2.ToolBar {
                Layout.fillWidth: true
                visible: root.receiver.outputSwitchingAvailable
                position: QQC2.ToolBar.Footer

                RowLayout {
                    anchors.fill: parent
                    spacing: Kirigami.Units.smallSpacing

                    Kirigami.Icon {
                        source: "audio-speakers-symbolic"
                        implicitWidth: Kirigami.Units.iconSizes.smallMedium
                        implicitHeight: implicitWidth
                    }
                    QQC2.Label {
                        text: "Output"
                    }
                    QQC2.ComboBox {
                        id: outputBox
                        objectName: "outputPicker"
                        Layout.fillWidth: true
                        Layout.minimumWidth: Kirigami.Units.gridUnit * 6
                        Layout.preferredWidth: Kirigami.Units.gridUnit * 10
                        model: root.outputEntries
                        textRole: "label"
                        valueRole: "name"
                        currentIndex: root.outputIndex
                        // The list says which device the system default is; the field stays short.
                        displayText: currentIndex === 0 ? "Follow System" : currentText
                        onActivated: root.receiver.setOutput(currentValue)

                        QQC2.ToolTip.text: currentText
                        QQC2.ToolTip.visible: hovered && !popup.visible
                        QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay

                        Connections {
                            target: outputBox.popup
                            function onAboutToShow() {
                                root.receiver.refreshOutputs();
                            }
                        }
                    }
                }
            }
        }
    }
}
