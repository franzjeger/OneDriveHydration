// The Dolphin overlay plugin: a per-file "on device" / "cloud only" emblem for
// files inside the OneDrive sync root. It is the file-manager half of what the
// tray shows in aggregate — which files hold their content locally, and which
// are placeholders the cloud still owns.
//
// The whole answer for one file is a single presence probe of the framework's
// dehydrated mark (see below). That read is metadata, not content: it fires no
// fanotify pre-content event and cannot hydrate the file it is drawing a badge
// for. Measured on a real mount under a live mark, on btrfs, ext4, and xfs, by
// HydrationAPI's probes/xattrread.c — zero events, every time. This is the load
// bearing safety property: a status UI that read *content* would hydrate every
// one of ~166k placeholders the moment the user opened a folder, or deadlock
// against the very event it triggered (HydrationAPI DESIGN.md 6a-ter). This
// plugin only ever issues lgetxattr, never open()/read()/mmap.
//
// KOverlayIconPlugin (KIO) is the mechanism KDE file managers use for
// third-party overlay emblems, and the one Nextcloud's current Dolphin
// integration uses. getOverlays is called on the main thread for the items
// Dolphin is drawing — tens to hundreds, never the whole tree — so the cost is
// O(files the user is looking at). The header requires it not block and
// recommends a cache; we answer from an in-process cache and push a miss to a
// worker thread, emitting overlaysChanged when the answer is ready.
#include <KOverlayIconPlugin>

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QHash>
#include <QPointer>
#include <QRunnable>
#include <QStandardPaths>
#include <QStringList>
#include <QTextStream>
#include <QThreadPool>
#include <QUrl>

#include <sys/xattr.h>
#include <cerrno>

namespace
{
// The framework's placeholder mark. Presence = cloud-only; absence = resident.
// Kept byte-for-byte identical to hydration_protocol::xattr::DEHYDRATED; the
// packaging test dolphin_overlay_package.rs fails if the two ever drift.
constexpr const char *kDehydratedXattr = "user.hydration.dehydrated";

// The cloud-only emblem, a Breeze built-in resolved through the desktop icon
// theme — it ships with every KF6 desktop (measured present in breeze and
// breeze-dark), so the feature draws something real with no icon-install step. A
// later slice can swap in a branded onedrive-cloud; getOverlays takes any icon
// name, so that is a one-line change. There is deliberately no resident emblem:
// a check on every on-device file (and every folder) is noise that reads as
// "everything is downloaded" — only the not-here files are marked.
constexpr const char *kCloudOnlyEmblem = "vcs-update-required"; // placeholder

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
    HydrationOverlayPlugin() { loadRoots(); }

    QStringList getOverlays(const QUrl &item) override
    {
        if (!item.isLocalFile())
            return {};
        const QString path = item.toLocalFile();
        if (!isUnderRoot(path))
            return {}; // not our folder: draw nothing, badge nobody else's files

        const auto cached = m_cache.constFind(path);
        if (cached != m_cache.constEnd())
            return cached.value();

        // Miss: answer empty now (the header forbids blocking the main thread),
        // and compute the one lgetxattr on a worker. When it lands we cache it
        // and emit overlaysChanged so Dolphin re-queries just this item. The
        // worker touches no member state; the result is marshalled back to this
        // (main) thread, so the cache is only ever read or written here and needs
        // no lock.
        QPointer<HydrationOverlayPlugin> self(this);
        const QByteArray local = QFile::encodeName(path);
        QThreadPool::globalInstance()->start(QRunnable::create([self, path, local]() {
            const QStringList overlays = probe(local);
            // qApp outlives every plugin, so it is a safe marshalling context;
            // the QPointer guards the plugin itself having been destroyed.
            QMetaObject::invokeMethod(
                qApp,
                [self, path, overlays]() {
                    if (self)
                        self->deliver(path, overlays);
                },
                Qt::QueuedConnection);
        }));
        return {};
    }

private:
    // The one metadata probe, run on the worker thread. lgetxattr, not getxattr,
    // so a symlink is not followed into a read of its target; NULL/0 so no value
    // is fetched — presence is the whole signal.
    static QStringList probe(const QByteArray &local)
    {
        // Only a cloud-only placeholder gets an emblem — a cloud saying "not on
        // this device yet". Everything else draws NOTHING: a resident file, and
        // (crucially) a directory, both answer ENODATA, and marking every local
        // file and every folder with a check reads as "everything is downloaded"
        // — the wrong signal in a tree that is mostly placeholders. The useful
        // mark is the one on what is NOT here, so that is the only one drawn.
        // ENOTSUP (no xattrs), ENOENT (raced deletion): also nothing.
        const ssize_t r = lgetxattr(local.constData(), kDehydratedXattr, nullptr, 0);
        if (r >= 0)
            return {QString::fromLatin1(kCloudOnlyEmblem)}; // cloud-only placeholder
        return {};
    }

    void deliver(const QString &path, const QStringList &overlays)
    {
        m_cache.insert(path, overlays);
        Q_EMIT overlaysChanged(QUrl::fromLocalFile(path), overlays);
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

    QHash<QString, QStringList> m_cache;
    QStringList m_roots;
};

#include "hydrationoverlay.moc"
