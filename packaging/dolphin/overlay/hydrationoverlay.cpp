// SPDX-License-Identifier: MIT OR Apache-2.0
// Dolphin file availability and local-content status. Only metadata is read:
// reading a placeholder's content would download it just to draw its icon.
// A clean content stamp is not proof that pending renames/deletions or remote
// changes have settled. These emblems do not claim complete namespace sync.
#include <KOverlayIconPlugin>
#include "selection.h"

#include <QDBusConnection>
#include <QFile>
#include <QFileSystemWatcher>
#include <QHash>
#include <QStandardPaths>
#include <QStringList>
#include <QTextStream>
#include <QUrl>

#include <sys/stat.h>
#include <sys/xattr.h>
#include <dirent.h>
#include <cerrno>
#include <cstring>

namespace
{
// The framework's placeholder mark. Presence = cloud-only; absence = resident.
// Kept byte-for-byte identical to hydration_protocol::xattr::DEHYDRATED; the
// packaging test dolphin_overlay_package.rs fails if the two ever drift.
constexpr const char *kDehydratedXattr = "user.hydration.dehydrated";

// Use the framework's durable content stamp, not residency alone. Its format
// is defined by hydration_protocol::stamp::of: <mtime_sec>.<mtime_nsec>:<size>.
constexpr const char *kStampXattr = "user.hydration.stamp";
constexpr const char *kIdXattr = "user.hydration.id";
constexpr const char *kTagXattr = "user.hydration.etag";
constexpr const char *kCloudOnlyEmblem = "cloud-download";
constexpr const char *kOnDeviceEmblem = "emblem-success";
constexpr const char *kChangedEmblem = "view-refresh";
constexpr const char *kUnknownEmblem = "emblem-question";

enum class ContentState { CloudOnly, Clean, Changed, Attention, Unknown, Unsupported };

QStringList emblems(ContentState state)
{
    switch (state) {
    case ContentState::CloudOnly: return {QString::fromLatin1(kCloudOnlyEmblem)};
    case ContentState::Clean: return {QString::fromLatin1(kOnDeviceEmblem)};
    case ContentState::Attention: return {QStringLiteral("dialog-warning")};
    case ContentState::Changed: return {QString::fromLatin1(kChangedEmblem)};
    case ContentState::Unknown: return {QString::fromLatin1(kUnknownEmblem)};
    case ContentState::Unsupported: return {};
    }
    return {};
}

// Where the sync roots are listed, one absolute path per line, written by
// install-overlay.sh. The plugin only badges files under a configured root, so
// a cloud-only placeholder anywhere else on the system (another sync client's,
// say) is left alone — the roots are what scope the emblem to this OneDrive.
QString rootsConfigPath()
{
    const QString base = QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation);
    return base + QStringLiteral("/onedrive-hydration/overlay-roots");
}
} // namespace

class HydrationOverlayPlugin : public KOverlayIconPlugin
{
    Q_PLUGIN_METADATA(IID "org.kde.overlayicon.onedrivehydration" FILE "hydrationoverlay.json")
    Q_OBJECT

public:
    HydrationOverlayPlugin()
    {
        loadRoots();
        // Upload completion changes the stamp without changing size or mtime.
        // Watch requested items so their badges refresh on metadata changes too.
        connect(&m_watcher, &QFileSystemWatcher::fileChanged, this, &HydrationOverlayPlugin::metadataChanged);
        connect(&m_watcher, &QFileSystemWatcher::directoryChanged, this, &HydrationOverlayPlugin::metadataChanged);
        // Listen for the servicemenu wrappers' change announcement and PUSH a
        // fresh overlay for each affected item. This is the KDE-blessed refresh
        // path — KFileItemModelRolesUpdater connects to overlaysChanged — and the
        // only one that survives an eviction: KIO's own re-query is stat-keyed and
        // skips a file whose size and mtime did not move, so Free Up Space would
        // otherwise leave its old badge until a manual F5. Empty service name =
        // any sender, so it catches the wrappers' broadcast dbus-send.
        QDBusConnection::sessionBus().connect(QString(),
                                              QStringLiteral("/org/kde/KDirNotify"),
                                              QStringLiteral("org.kde.KDirNotify"),
                                              QStringLiteral("FilesChanged"),
                                              this,
                                              SLOT(onFilesChanged(QStringList)));
    }

    QStringList getOverlays(const QUrl &item) override
    {
        if (!item.isLocalFile())
            return {};
        const QString path = item.toLocalFile();
        if (!isUnderRoot(path))
            return {}; // not our folder: badge nobody else's files

        // Re-read metadata instead of caching content state. Directory work is
        // bounded below because this runs on Dolphin's UI thread.
        watch(path);
        return badges(path);
    }

private Q_SLOTS:
    // A wrapper changed a file's residency and announced it over KDirNotify. For
    // every announced URL under one of our roots, push the fresh badge so Dolphin
    // repaints it now — no stat change required, unlike KIO's own refresh.
    void onFilesChanged(const QStringList &urls)
    {
        for (const QString &u : urls) {
            const QUrl url(u);
            if (!url.isLocalFile())
                continue;
            const QString path = url.toLocalFile();
            if (!isUnderRoot(path))
                continue;
            // A child's residency/content state also changes its ancestors'
            // aggregate. Stop at the configured root, without following links.
            QString changed = path;
            while (isUnderRoot(changed)) {
                Q_EMIT overlaysChanged(QUrl::fromLocalFile(changed), badges(changed));
                changed = changed.left(changed.lastIndexOf(QLatin1Char('/')));
            }
        }
    }

private:
    QStringList badges(const QString &path) const
    {
        for (const auto &root : m_roots) {
            if (path != root && !path.startsWith(root + '/')) continue;
            switch (selectionState(root, path)) {
            case SelectionState::Excluded: return {QStringLiteral("onedrive-hydration-excluded")};
            case SelectionState::Unknown: return {QStringLiteral("emblem-question")};
            case SelectionState::Synced: break;
            }
        }
        const auto state = probe(QFile::encodeName(path));
        if (state == ContentState::Clean) {
            QString current = path;
            while (isUnderRoot(current)) {
                if (lgetxattr(QFile::encodeName(current).constData(), "user.hydration.pinned", nullptr, 0) >= 0)
                    return {QStringLiteral("onedrive-hydration-pinned")};
                current = current.left(current.lastIndexOf('/'));
            }
        }
        return emblems(state);
    }

    void watch(const QString &path)
    {
        // Root selection changes must remain watched even after many listings.
        if (m_roots.contains(path)) {
            if (!m_watcher.directories().contains(path)) m_watcher.addPath(path);
            return;
        }
        // Keep resource use bounded when navigating many folders. A relisted
        // item renews its place; no recursive watch or background tree scan.
        if (!m_watched.removeOne(path)) {
            if (m_watched.size() >= 512) {
                const auto old = m_watched.takeFirst();
                m_watcher.removePath(old);
                m_pins.remove(old);
            }
            if (!m_watcher.addPath(path))
                return;
            m_pins.insert(path, lgetxattr(QFile::encodeName(path).constData(), "user.hydration.pinned", nullptr, 0) >= 0);
        }
        m_watched.append(path);
    }

    void metadataChanged(const QString &path)
    {
        if (m_roots.contains(path))
            for (const auto &child : std::as_const(m_watched))
                Q_EMIT overlaysChanged(QUrl::fromLocalFile(child), badges(child));
        const bool pinned = lgetxattr(QFile::encodeName(path).constData(), "user.hydration.pinned", nullptr, 0) >= 0;
        if (m_pins.value(path) != pinned) {
            m_pins.insert(path, pinned);
            for (const auto &child : std::as_const(m_watched))
                if (child.startsWith(path + '/')) Q_EMIT overlaysChanged(QUrl::fromLocalFile(child), badges(child));
        }
        // Qt drops a file watch after deletion/replacement; allow re-adding it
        // when Dolphin relists the replacement.
        if (!m_watcher.files().contains(path) && !m_watcher.directories().contains(path))
            m_watched.removeOne(path);
        onFilesChanged({QUrl::fromLocalFile(path).toString()});
    }

    static QByteArray stampOf(const struct stat &st)
    {
        return QByteArray::number(static_cast<qlonglong>(st.st_mtim.tv_sec)) + '.'
            + QByteArray::number(static_cast<qlonglong>(st.st_mtim.tv_nsec)) + ':'
            + QByteArray::number(static_cast<qlonglong>(st.st_size));
    }

    static ContentState probeFile(const QByteArray &local)
    {
        const ssize_t dehydrated = lgetxattr(local.constData(), kDehydratedXattr, nullptr, 0);
        const bool cloudOnly = dehydrated >= 0;
        if (!cloudOnly && errno != ENODATA)
            return ContentState::Unknown;

        char recorded[96];
        const ssize_t length = lgetxattr(local.constData(), kStampXattr, recorded, sizeof(recorded));
        if (length <= 0 || lgetxattr(local.constData(), kIdXattr, nullptr, 0) <= 0
            || lgetxattr(local.constData(), kTagXattr, nullptr, 0) <= 0)
            return ContentState::Unknown;

        struct stat st;
        if (lstat(local.constData(), &st) != 0 || !S_ISREG(st.st_mode))
            return ContentState::Unknown;
        if (QByteArray(recorded, length) != stampOf(st))
            return cloudOnly ? ContentState::Attention : ContentState::Changed;
        return cloudOnly ? ContentState::CloudOnly : ContentState::Clean;
    }

    ContentState probe(const QByteArray &local) const
    {
        struct stat st;
        if (lstat(local.constData(), &st) != 0)
            return ContentState::Unknown;
        if (S_ISREG(st.st_mode))
            return probeFile(local);
        if (S_ISDIR(st.st_mode))
            return probeDirectory(local);
        return ContentState::Unsupported;
    }

    // This runs on Dolphin's UI thread. Count every entry, including symlinks
    // and directories, so a large directory cannot bypass the work limit. A
    // partial sample must never turn into an "everything is clean" checkmark.
    ContentState probeDirectory(const QByteArray &local) const
    {
        constexpr int kMaxEntries = 128;
        constexpr int kMaxDepth = 4;
        int checked = 0;
        bool cloud = false;
        bool unknown = false;
        bool complete = true;
        QList<QPair<QByteArray, int>> queue{{local, 0}};
        for (qsizetype head = 0; head < queue.size(); ++head) {
            const auto entry = queue.at(head);
            if (checked >= kMaxEntries) { complete = false; break; }
            if (lgetxattr(entry.first.constData(), kIdXattr, nullptr, 0) <= 0)
                unknown = true;
            DIR *directory = opendir(entry.first.constData());
            if (!directory) { unknown = true; continue; }
            for (;;) {
                errno = 0;
                struct dirent *e = readdir(directory);
                if (!e) {
                    if (errno != 0) unknown = true;
                    break;
                }
                if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0)
                    continue;
                if (++checked > kMaxEntries) { complete = false; break; }
                if (strncmp(e->d_name, ".hydration-", 11) == 0
                    || strncmp(e->d_name, ".onedrive-", 10) == 0)
                    continue;
                const QByteArray child = entry.first + '/' + e->d_name;
                bool excluded = false;
                const QString childPath = QFile::decodeName(child);
                for (const auto &root : m_roots) {
                    if (!childPath.startsWith(root + '/')) continue;
                    const auto selection = selectionState(root, childPath);
                    if (selection == SelectionState::Excluded) excluded = true;
                    if (selection == SelectionState::Unknown) unknown = true;
                }
                if (excluded) continue;
                struct stat st;
                if (lstat(child.constData(), &st) != 0) { unknown = true; continue; }
                if (S_ISDIR(st.st_mode)) {
                    if (entry.second + 1 >= kMaxDepth) complete = false;
                    else queue.append({child, entry.second + 1});
                } else if (S_ISREG(st.st_mode)) {
                    const ContentState state = probeFile(child);
                    if (state == ContentState::Changed || state == ContentState::Attention) {
                        closedir(directory);
                        return state;
                    }
                    if (state == ContentState::CloudOnly) cloud = true;
                    if (state == ContentState::Unknown) unknown = true;
                } else {
                    // Symlinks and special files have no supported content
                    // state. They cannot justify a green folder checkmark.
                    unknown = true;
                }
            }
            closedir(directory);
        }
        if (cloud) return ContentState::CloudOnly;
        if (!complete || unknown) return ContentState::Unknown;
        return ContentState::Clean;
    }

    void loadRoots()
    {
        m_roots.clear();
        QFile f(rootsConfigPath());
        if (!f.open(QIODevice::ReadOnly | QIODevice::Text))
            return;
        QTextStream in(&f);
        while (!in.atEnd()) {
            QString line = in.readLine().trimmed();
            if (line.isEmpty() || line.startsWith(QLatin1Char('#')))
                continue;
            // Store without a trailing slash; isUnderRoot re-adds one so that a
            // root of /home/u/OneDrive matches /home/u/OneDrive/x but not a
            // sibling /home/u/OneDriveBackup.
            while (line.endsWith(QLatin1Char('/')) && line.size() > 1)
                line.chop(1);
            m_roots.append(line);
            m_watcher.addPath(line);
        }
    }

    bool isUnderRoot(const QString &path) const
    {
        for (const QString &root : m_roots) {
            if (path == root)
                return true;
            if (path.startsWith(root + QLatin1Char('/')))
                return true;
        }
        return false;
    }

    QStringList m_roots;
    QFileSystemWatcher m_watcher;
    QStringList m_watched;
    QHash<QString, bool> m_pins;
};

#include "hydrationoverlay.moc"
