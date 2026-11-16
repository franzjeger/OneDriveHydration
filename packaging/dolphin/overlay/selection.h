// SPDX-License-Identifier: MIT OR Apache-2.0
#pragma once
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QString>
#include <QStringList>
#include <cerrno>
#include <sys/xattr.h>

enum class SelectionState { Synced, Excluded, Unknown };

struct RootSelection {
    bool valid = true;
    QStringList excludedPrefixes;

    SelectionState classify(const QString &path) const
    {
        if (!valid) return SelectionState::Unknown;
        for (const auto &prefix : excludedPrefixes) {
            if (path == prefix || path.startsWith(prefix + QLatin1Char('/')))
                return SelectionState::Excluded;
        }
        return SelectionState::Synced;
    }
};

inline RootSelection loadRootSelection(const QString &root)
{
    RootSelection out;
    const QByteArray name = QFile::encodeName(root);
    char bytes[16001];
    const auto count = lgetxattr(name.constData(), "user.hydration.selection", bytes, sizeof(bytes));
    if (count < 0) {
        out.valid = (errno == ENODATA);
        return out;
    }
    if (count > 16000) {
        out.valid = false;
        return out;
    }
    const auto document = QJsonDocument::fromJson(QByteArray(bytes, count));
    if (!document.isArray()) {
        out.valid = false;
        return out;
    }
    const auto array = document.array();
    out.excludedPrefixes.reserve(array.size());
    for (const auto &value : array) {
        if (!value.isString() || value.toString().isEmpty()) {
            out.valid = false;
            out.excludedPrefixes.clear();
            return out;
        }
        out.excludedPrefixes.append(root + QLatin1Char('/') + value.toString());
    }
    return out;
}

inline SelectionState selectionState(const QString &root, const QString &path)
{
    return loadRootSelection(root).classify(path);
}
