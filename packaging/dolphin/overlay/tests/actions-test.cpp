// SPDX-License-Identifier: MIT OR Apache-2.0
#include <KAbstractFileItemActionPlugin>
#include <KFileItem>
#include <KFileItemListProperties>
#include <KPluginFactory>
#include <KPluginMetaData>
#include <QApplication>
#include <QDir>
#include <QFile>
#include <QMenu>
#include <QTemporaryDir>
#include <cstdio>
#include <sys/xattr.h>

int main(int argc, char **argv)
{
    QTemporaryDir scratch;
    qputenv("XDG_CONFIG_HOME", scratch.path().toUtf8());
    QApplication app(argc, argv);
    if (argc != 2) return 2;
    const QString root = scratch.path() + "/drive";
    QDir().mkpath(root + "/folder");
    QDir().mkpath(scratch.path() + "/onedrive-hydration");
    QFile roots(scratch.path() + "/onedrive-hydration/overlay-roots");
    if (!roots.open(QIODevice::WriteOnly)) return 2;
    roots.write(root.toUtf8() + '\n'); roots.close();
    QFile file(root + "/file.txt");
    if (!file.open(QIODevice::WriteOnly)) return 2;
    file.close();
    auto result = KPluginFactory::instantiatePlugin<KAbstractFileItemActionPlugin>(KPluginMetaData(QString::fromLocal8Bit(argv[1])));
    if (!result.plugin) { fprintf(stderr, "%s\n", qPrintable(result.errorString)); return 1; }
    auto check = [&](const QStringList &paths, bool expected) {
        KFileItemList items;
        for (const auto &path : paths) items << KFileItem(QUrl::fromLocalFile(path));
        QMenu parent;
        const auto actions = result.plugin->actions(KFileItemListProperties(items), &parent);
        if (actions.isEmpty() == expected) return false;
        return !expected || (actions.size() == 1 && actions.first()->menu() && actions.first()->menu()->actions().size() == 2);
    };
    bool passed = check({root + "/file.txt"}, true) && check({root + "/folder"}, true)
        && check({root + "/file.txt", root + "/folder"}, true) && check({scratch.path()}, false)
        && check({root + "/file.txt", scratch.path()}, false) && check({root}, false);
    const QByteArray bytes = "[\"folder\"]";
    passed = passed && lsetxattr(QFile::encodeName(root).constData(), "user.hydration.selection", bytes.constData(), bytes.size(), 0) == 0;
    passed = passed && check({root + "/folder"}, false) && check({root + "/file.txt"}, true);
    delete result.plugin;
    if (!passed) { fprintf(stderr, "FAIL: OneDrive menu selection/scoping\n"); return 1; }
    fprintf(stdout, "PASS: real menu plugin accepts files, folders and mixed selections only inside OneDrive\n");
    return 0;
}
