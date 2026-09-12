// SPDX-License-Identifier: MIT OR Apache-2.0
import QtQml

QtObject {
    id: log
    property var entries: []
    property var activeUploads: []

    function add(title, detail, icon, relative) {
        entries = [{title: title, detail: detail, icon: icon,
            path: relative || "", time: new Date()}].concat(entries).slice(0, 20);
    }

    function observeUploads(paths, record) {
        if (record) {
            paths.forEach(function(path) {
                if (log.activeUploads.indexOf(path) < 0)
                    log.add(path.split("/").pop(), "Upload started", "document-send", path);
            });
        }
        // A disappearance may mean success, retry, or cancellation. The
        // current protocol cannot distinguish these, so record no outcome.
        activeUploads = paths.slice();
    }
}
