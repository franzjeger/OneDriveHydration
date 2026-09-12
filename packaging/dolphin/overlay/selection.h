// SPDX-License-Identifier: MIT OR Apache-2.0
#pragma once
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QString>
#include <cerrno>
#include <sys/xattr.h>

enum class SelectionState { Synced, Excluded, Unknown };
inline SelectionState selectionState(const QString &root, const QString &path)
{
    const QByteArray name = QFile::encodeName(root);
    char bytes[16001];
    const auto count = lgetxattr(name.constData(), "user.hydration.selection", bytes, sizeof(bytes));
    if (count < 0) return errno == ENODATA ? SelectionState::Synced : SelectionState::Unknown;
    if (count > 16000) return SelectionState::Unknown;
    const auto document = QJsonDocument::fromJson(QByteArray(bytes, count));
    if (!document.isArray()) return SelectionState::Unknown;
    for (const auto &value : document.array()) {
        if (!value.isString() || value.toString().isEmpty()) return SelectionState::Unknown;
        const QString prefix = root + '/' + value.toString();
        if (path == prefix || path.startsWith(prefix + '/')) return SelectionState::Excluded;
    }
    return SelectionState::Synced;
}
