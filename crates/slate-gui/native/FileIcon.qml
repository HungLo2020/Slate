import QtQuick
import Slate.Native

// A file browser row's icon: the desktop theme's icon for the entry's type,
// or a small drawn page/folder when the theme has none.
Item {
    id: icon
    property string name: ""
    property bool directory: false
    property bool expanded: false
    property bool parentLink: false
    property color tint: Theme.textColor
    property real extent: 16
    implicitWidth: extent
    implicitHeight: extent

    // Freedesktop icon names by extension; the first one the theme has wins.
    readonly property var byExtension: ({
            "rs": ["text-rust", "text-x-rust"],
            "c": ["text-x-csrc"],
            "h": ["text-x-chdr"],
            "cc": ["text-x-c++src"],
            "cpp": ["text-x-c++src"],
            "cxx": ["text-x-c++src"],
            "hh": ["text-x-c++hdr"],
            "hpp": ["text-x-c++hdr"],
            "py": ["text-x-python", "text-x-python3"],
            "js": ["application-javascript", "text-javascript"],
            "mjs": ["application-javascript", "text-javascript"],
            "cjs": ["application-javascript", "text-javascript"],
            "ts": ["application-typescript", "text-typescript"],
            "json": ["application-json"],
            "md": ["text-markdown", "text-x-markdown"],
            "html": ["text-html"],
            "htm": ["text-html"],
            "css": ["text-css"],
            "xml": ["application-xml", "text-xml"],
            "svg": ["image-svg+xml"],
            "toml": ["application-toml", "text-x-toml"],
            "yaml": ["application-x-yaml", "text-x-yaml"],
            "yml": ["application-x-yaml", "text-x-yaml"],
            "sh": ["application-x-shellscript", "text-x-script"],
            "bash": ["application-x-shellscript", "text-x-script"],
            "zsh": ["application-x-shellscript", "text-x-script"],
            "fish": ["application-x-shellscript", "text-x-script"],
            "qml": ["text-x-qml"],
            "go": ["text-x-go"],
            "java": ["text-x-java"],
            "rb": ["application-x-ruby", "text-x-ruby"],
            "lua": ["text-x-lua"],
            "sql": ["application-sql", "text-x-sql"],
            "png": ["image-png", "image-x-generic"],
            "jpg": ["image-jpeg", "image-x-generic"],
            "jpeg": ["image-jpeg", "image-x-generic"],
            "gif": ["image-gif", "image-x-generic"],
            "webp": ["image-webp", "image-x-generic"],
            "ico": ["image-x-ico", "image-x-generic"],
            "pdf": ["application-pdf"],
            "zip": ["application-zip", "package-x-generic"],
            "gz": ["application-gzip", "package-x-generic"],
            "xz": ["application-x-xz", "package-x-generic"],
            "zst": ["application-zstd", "package-x-generic"],
            "tar": ["application-x-tar", "package-x-generic"],
            "txt": ["text-plain", "text-x-generic"]
        })
    readonly property var byName: ({
            "makefile": ["text-x-makefile"],
            "cmakelists.txt": ["text-x-cmake"],
            "license": ["text-x-copying"],
            "copying": ["text-x-copying"],
            "readme": ["text-x-readme"],
            "readme.md": ["text-markdown", "text-x-readme"]
        })
    function candidates() {
        if (parentLink)
            return ["go-up"];
        if (directory)
            return name === ".git" ? ["folder-git", "folder"] : expanded ? ["folder-open", "folder"] : ["folder"];
        var lower = name.toLowerCase();
        var dot = lower.lastIndexOf(".");
        var list = (byName[lower] || []).concat(dot > 0 ? byExtension[lower.substring(dot + 1)] || [] : []);
        return list.concat(["text-x-generic", "unknown"]);
    }
    // Qt caches theme lookups, so rows can ask as they are created.
    readonly property string themeName: {
        var list = candidates();
        for (var i = 0; i < list.length; i++)
            if (slate.hasIcon(list[i]))
                return list[i];
        return "";
    }

    Image {
        id: themed
        anchors.centerIn: parent
        visible: icon.themeName.length > 0 && status === Image.Ready
        // Symbolic navigation icons follow the text color; file types keep theirs.
        source: icon.themeName.length ? "image://icon/" + icon.themeName + (icon.parentLink ? "?" + icon.tint : "") : ""
        sourceSize: Qt.size(icon.extent, icon.extent)
        width: icon.extent
        height: icon.extent
        smooth: true
    }

    // Drawn fallbacks, in proportion to the row's text.
    Item {
        anchors.fill: parent
        visible: !themed.visible
        // Folder: a tab and a body.
        Rectangle {
            visible: icon.directory && !icon.parentLink
            x: icon.extent * 0.08
            y: icon.extent * 0.2
            width: icon.extent * 0.4
            height: icon.extent * 0.2
            radius: 1
            color: Qt.alpha(icon.tint, 0.55)
        }
        Rectangle {
            visible: icon.directory && !icon.parentLink
            x: icon.extent * 0.08
            y: icon.extent * 0.3
            width: icon.extent * 0.84
            height: icon.extent * 0.55
            radius: 2
            color: Qt.alpha(icon.tint, icon.expanded ? 0.3 : 0.45)
            border.width: 1
            border.color: Qt.alpha(icon.tint, 0.6)
        }
        // File: an outlined page.
        Rectangle {
            visible: !icon.directory && !icon.parentLink
            x: icon.extent * 0.2
            y: icon.extent * 0.1
            width: icon.extent * 0.6
            height: icon.extent * 0.8
            radius: 1.5
            color: "transparent"
            border.width: 1
            border.color: Qt.alpha(icon.tint, 0.6)
        }
        Text {
            visible: icon.parentLink
            anchors.centerIn: parent
            text: "↑"
            textFormat: Text.PlainText
            color: icon.tint
            font.pixelSize: icon.extent
        }
    }
}
