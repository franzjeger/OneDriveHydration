// The Dolphin overlay plugin: a per-file "on device" / "cloud only" emblem for
// files inside the OneDrive sync root. It is the file-manager half of what the
// tray shows in aggregate — which files hold their content locally, and which
// are placeholders the cloud still owns.
//
// The answer for one file is a presence probe of the framework's dehydrated
// mark, and for an unmarked file one lstat to tell a resident file from a folder
// (see below). Both are metadata, not content: they fire no fanotify pre-content
// event and cannot hydrate the file being badged. Measured on a real mount under
// a live mark, on btrfs, ext4, and xfs, by HydrationAPI's probes/xattrread.c —
// zero events, every time. This is the load-bearing safety property: a status UI
// that read *content* would hydrate every one of ~166k placeholders the moment
// the user opened a folder, or deadlock against the very event it triggered
// (HydrationAPI DESIGN.md 6a-ter). This plugin only ever issues lgetxattr and
// lstat, never open()/read()/mmap.
//
// KOverlayIconPlugin (KIO) is the mechanism KDE file managers use for
// third-party overlay emblems, and the one Nextcloud's current Dolphin
// integration uses. getOverlays is called on the main thread for the items
// Dolphin is drawing — tens to hundreds, never the whole tree — so the cost is
// O(files the user is looking at). The header requires it not block and
// recommends a cache; we answer synchronously with one lgetxattr and
// deliberately do NOT cache. The probe is a metadata read of a local file
// (microseconds, and P1 measured it fires no pre-content event), so it does not
// meaningfully block; and not caching is what keeps the emblem correct after
// Free Up Space or Keep on Device changes a file's residency — a path-keyed
// cache went on returning the stale answer Dolphin drew before the change.
#include <KOverlayIconPlugin>

#include <QDBusConnection>
#include <QFile>
#include <QStandardPaths>
#include <QStringList>
#include <QTextStream>
#include <QUrl>

#include <sys/stat.h>
#include <sys/xattr.h>
#include <cerrno>

namespace
{
// The framework's placeholder mark. Presence = cloud-only; absence = resident.
// Kept byte-for-byte identical to hydration_protocol::xattr::DEHYDRATED; the
// packaging test dolphin_overlay_package.rs fails if the two ever drift.
constexpr const char *kDehydratedXattr = "user.hydration.dehydrated";

// The two state emblems, Breeze built-ins resolved through the desktop icon
// theme (measured present in breeze and breeze-dark), so the feature draws
// something real with no icon-install step. The pairing mirrors what a Windows
// OneDrive user already reads at a glance: a cloud for "in the cloud, not here",
// a green check for "on this device".
//
// A FILE always carries one or the other. A FOLDER carries the check only when
// it is pinned — Kept on Device — so "sync this folder to the device" leaves a
// visible mark and "Free Up Space" clears it; an unpinned folder draws nothing,
// because one probe cannot know whether its whole subtree is resident, and a
// check on EVERY (mostly-cloud) folder was the "everything is downloaded" misread
// an earlier cut produced.
//
// The history is the design: cut one badged files AND every folder with a check,
// and read as "everything downloaded"; cut two badged only cloud-only files and
// left residents bare, so a glance could not tell "downloaded" from "unmarked".
// So both states carry a distinct mark, and a folder is marked only when the user
// pinned it. A later slice can swap in branded icons; getOverlays takes any name.
constexpr const char *kCloudOnlyEmblem = "cloud-download"; // a cloud: not here yet
constexpr const char *kOnDeviceEmblem = "emblem-success";  // green check: on device
// The pin Keep on Device sets on a folder (and Free Up Space clears). Same
// namespace as the dehydrated mark; a presence probe, never a value read.
constexpr const char *kPinnedXattr = "user.hydration.pinned";

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

        // Answer synchronously with one lgetxattr, and deliberately do NOT cache.
        // The probe is a metadata read of a local file — microseconds, and P1
        // measured it fires no pre-content event — so it does not meaningfully
        // block the main thread. Not caching is what keeps the emblem correct
        // after Free Up Space or Keep on Device changes a file's residency:
        // Dolphin re-queries the item on its next relist (the servicemenu
        // wrappers nudge one via org.kde.KDirNotify), and a fresh probe reflects
        // the new state. A path-keyed cache went on returning the stale answer —
        // the "nothing happens when I click" the first cut showed.
        return probe(QFile::encodeName(path));
    }

private Q_SLOTS:
    // A wrapper changed a file's residency and announced it over KDirNotify. For
    // every announced URL under one of our roots, push the fresh badge so Dolphin
    // repaints it now — no stat change required, unlike KIO's own refresh. probe
    // re-reads the mark, so Keep on Device shows the check and Free Up Space the
    // cloud, the moment the wrapper finishes.
    void onFilesChanged(const QStringList &urls)
    {
        for (const QString &u : urls) {
            const QUrl url(u);
            if (!url.isLocalFile())
                continue;
            const QString path = url.toLocalFile();
            if (!isUnderRoot(path))
                continue;
            Q_EMIT overlaysChanged(url, probe(QFile::encodeName(path)));
        }
    }

private:
    // The metadata probe: at most two event-free syscalls, lgetxattr then (only
    // for something with no mark) lstat. lgetxattr/lstat, not getxattr/stat, so a
    // symlink is not followed into a read of its target; NULL/0 so no xattr value
    // is fetched — presence of the mark is the whole cloud/resident signal. Both
    // read metadata only: no content, no hydration.
    static QStringList probe(const QByteArray &local)
    {
        // A placeholder carries the dehydrated mark: cloud-only, and always a
        // regular file. Draw the cloud.
        const ssize_t r = lgetxattr(local.constData(), kDehydratedXattr, nullptr, 0);
        if (r >= 0)
            return {QString::fromLatin1(kCloudOnlyEmblem)};

        // No mark: a resident file, OR a directory, OR something with no xattrs /
        // raced away (ENOTSUP/ENOENT). lstat draws the lines below; a failed stat
        // (raced deletion) draws nothing.
        struct stat st;
        if (lstat(local.constData(), &st) != 0)
            return {};

        // A resident regular FILE gets the on-device check.
        if (S_ISREG(st.st_mode))
            return {QString::fromLatin1(kOnDeviceEmblem)};

        // A FOLDER gets it only when pinned — Kept on Device. That is the one
        // folder state a single probe can know for sure, and it is exactly the
        // "I synced this folder to the device" the check is meant to confirm. An
        // unpinned folder draws nothing (its subtree residency is unknown, and a
        // check on every folder is the "everything is downloaded" misread).
        if (S_ISDIR(st.st_mode) && lgetxattr(local.constData(), kPinnedXattr, nullptr, 0) >= 0)
            return {QString::fromLatin1(kOnDeviceEmblem)};

        return {};
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
        }
    }

    bool isUnderRoot(const QString &path) const
    {
        for (const QString &root : m_roots) {
            if (path == root)
                return false; // the root itself is a folder, not a file in it
            if (path.startsWith(root + QLatin1Char('/')))
                return true;
        }
        return false;
    }

    QStringList m_roots;
};

#include "hydrationoverlay.moc"
