// SPDX-License-Identifier: MIT OR Apache-2.0
// Load the real plugin against scratch metadata, never the user's sync mount.
#include <KOverlayIconPlugin>
#include <QCoreApplication>
#include <QDir>
#include <QEventLoop>
#include <QFile>
#include <QPluginLoader>
#include <QTemporaryDir>
#include <QTimer>
#include <QUrl>
#include <cstdio>
#include <cstdlib>
#include <sys/stat.h>
#include <sys/xattr.h>
#include <unistd.h>

void require(bool condition, const char *message)
{
    if (!condition) { fprintf(stderr, "FAIL: %s\n", message); std::exit(1); }
}
void attribute(const QString &path, const char *name, const QByteArray &value)
{
    require(lsetxattr(QFile::encodeName(path).constData(), name, value.constData(), value.size(), 0) == 0, "write scratch attribute");
}
void identify(const QString &path)
{
    attribute(path, "user.hydration.id", "id");
    attribute(path, "user.hydration.etag", "ctag");
}
void stamp(const QString &path)
{
    struct stat st;
    require(lstat(QFile::encodeName(path).constData(), &st) == 0, "stat scratch file");
    const QByteArray value = QByteArray::number(static_cast<qlonglong>(st.st_mtim.tv_sec)) + '.'
        + QByteArray::number(static_cast<qlonglong>(st.st_mtim.tv_nsec)) + ':'
        + QByteArray::number(static_cast<qlonglong>(st.st_size));
    attribute(path, "user.hydration.stamp", value);
}
void file(const QString &path, bool clean = true)
{
    QFile f(path);
    require(f.open(QIODevice::WriteOnly), "create scratch file");
    require(f.write("original") == 8, "write scratch content");
    f.close();
    if (clean) { identify(path); stamp(path); }
}
void folder(const QString &path)
{
    require(QDir().mkpath(path), "create scratch folder");
    identify(path);
}

int main(int argc, char **argv)
{
    QTemporaryDir scratch;
    require(scratch.isValid(), "scratch directory");
    const QString root = scratch.path() + "/drive";
    const QString config = scratch.path() + "/config";
    qputenv("XDG_CONFIG_HOME", config.toUtf8());
    QCoreApplication app(argc, argv);
    require(argc == 2, "plugin path argument");
    folder(root);
    require(QDir().mkpath(config + "/onedrive-hydration"), "create config directory");
    QFile roots(config + "/onedrive-hydration/overlay-roots");
    require(roots.open(QIODevice::WriteOnly), "open roots config");
    roots.write(root.toUtf8() + '\n');
    roots.close();

    QPluginLoader loader(QString::fromLocal8Bit(argv[1]));
    auto *plugin = qobject_cast<KOverlayIconPlugin *>(loader.instance());
    if (!plugin) fprintf(stderr, "%s\n", qPrintable(loader.errorString()));
    require(plugin != nullptr, "load real overlay plugin");
    auto expect = [&](const QString &path, const QString &emblem) {
        const auto actual = plugin->getOverlays(QUrl::fromLocalFile(path));
        const auto expected = emblem.isEmpty() ? QStringList{} : QStringList{emblem};
        if (actual != expected) {
            fprintf(stderr, "FAIL: %s expected %s got %s\n", qPrintable(path), qPrintable(emblem), qPrintable(actual.join(',')));
            std::exit(1);
        }
    };

    file(root + "/clean.txt");
    expect(root + "/clean.txt", "emblem-success");
    expect(root, "emblem-success");
    QFile changed(root + "/clean.txt");
    require(changed.open(QIODevice::Append), "open scratch file for edit");
    changed.write("new edit");
    changed.close();
    expect(root + "/clean.txt", "view-refresh");
    expect(root, "view-refresh");
    stamp(root + "/clean.txt");
    expect(root + "/clean.txt", "emblem-success");
    attribute(root, "user.hydration.selection", "[\"clean.txt\"]");
    attribute(root + "/clean.txt", "user.hydration.stamp", "changed but excluded");
    expect(root + "/clean.txt", "onedrive-hydration-excluded");
    expect(root, "emblem-success");
    stamp(root + "/clean.txt");
    attribute(root, "user.hydration.selection", "[]");
    expect(root + "/clean.txt", "emblem-success");
    file(root + "/new.txt", false);
    expect(root + "/new.txt", "emblem-question");
    expect(root, "emblem-question");
    file(root + "/cloud.txt");
    attribute(root + "/cloud.txt", "user.hydration.dehydrated", "1");
    expect(root + "/cloud.txt", "cloud-download");
    expect(root, "cloud-download");
    attribute(root + "/cloud.txt", "user.hydration.stamp", "stale");
    expect(root + "/cloud.txt", "dialog-warning");
    expect(root, "dialog-warning");
    stamp(root + "/cloud.txt");
    require(lgetxattr(QFile::encodeName(root + "/cloud.txt").constData(), "user.hydration.dehydrated", nullptr, 0) == 1,
        "query preserves the placeholder mark");
    identify(root + "/new.txt");
    stamp(root + "/new.txt");

    folder(root + "/empty");
    expect(root + "/empty", "emblem-success");
    file(root + "/empty/pinned.txt");
    attribute(root + "/empty", "user.hydration.pinned", "1");
    expect(root + "/empty", "onedrive-hydration-pinned");
    expect(root + "/empty/pinned.txt", "onedrive-hydration-pinned");
    folder(root + "/deep");
    QString deep = root + "/deep";
    for (int i = 0; i < 6; ++i) { deep += "/child"; folder(deep); }
    file(deep + "/cloud.txt");
    attribute(deep + "/cloud.txt", "user.hydration.dehydrated", "1");
    expect(root + "/deep", "emblem-question");
    folder(root + "/large");
    for (int i = 0; i < 150; ++i) file(root + "/large/" + QString::number(i));
    expect(root + "/large", "emblem-question");

    const QString outside = scratch.path() + "/outside.txt";
    file(outside);
    expect(outside, "");
    folder(root + "-other");
    file(root + "-other/outside.txt");
    expect(root + "-other/outside.txt", "");
    require(symlink(QFile::encodeName(outside).constData(), QFile::encodeName(root + "/link").constData()) == 0, "scratch symlink");
    expect(root + "/link", "");
    expect(root + "/missing", "emblem-question");

    QStringList refreshed;
    QObject::connect(plugin, &KOverlayIconPlugin::overlaysChanged, [&](const QUrl &url, const QStringList &) { refreshed << url.toLocalFile(); });
    require(QMetaObject::invokeMethod(plugin, "onFilesChanged", Qt::DirectConnection,
        Q_ARG(QStringList, QStringList{QUrl::fromLocalFile(root + "/empty/file.txt").toString()})), "refresh signal handler");
    require(refreshed.contains(root + "/empty") && refreshed.contains(root), "child changes refresh folder badges");
    require(!refreshed.contains(scratch.path()), "refresh stays inside configured root");

    // An upload updates only xattrs: KIO's stat-based invalidation can miss it.
    // Verify the real filesystem watcher pushes the new badge without a relist.
    QCoreApplication::processEvents();
    attribute(root + "/clean.txt", "user.hydration.stamp", "old stamp");
    expect(root + "/clean.txt", "view-refresh");
    QCoreApplication::processEvents();
    QEventLoop loop;
    bool stampRefreshed = false;
    QObject::connect(plugin, &KOverlayIconPlugin::overlaysChanged, &loop,
        [&](const QUrl &url, const QStringList &badges) {
            if (url.toLocalFile() == root + "/clean.txt" && badges == QStringList{"emblem-success"}) {
                stampRefreshed = true;
                loop.quit();
            }
        });
    stamp(root + "/clean.txt");
    QTimer::singleShot(2000, &loop, &QEventLoop::quit);
    loop.exec();
    require(stampRefreshed, "metadata-only upload completion refreshes the badge");
    fprintf(stdout, "PASS: real plugin content states, folder bounds, root scoping, ancestor and metadata refresh\n");
    return 0;
}
