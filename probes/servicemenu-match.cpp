// Probe: does a KIO servicemenu declaring MimeType=all/allfiles actually
// reach the context menu of an arbitrary regular file, on this KIO build?
//
// This decides whether "Dolphin actions" can be shipped as data at all. The
// alternative — a compiled KFileItemActionPlugin — would put CMake, Qt6 and
// KF6 into a workspace that has twice refused a GUI toolkit and keeps
// `cargo deny` on an unchanged graph. all/allfiles is a documented KDE
// convention, but nothing installed on the measured machine used it, so it
// was an assumption and not a measurement until this ran.
//
// Four questions, and the last two shaped the design as much as the first:
//   1. Does an all/allfiles entry appear for a plain file at all?
//   2. Does it also appear for a *directory*? It must not: the daemon's
//      evict verb takes a file, and an action offered on a folder would
//      either do nothing or invite a bulk operation that does not exist.
//   3. Does it survive a multi-file selection, so Exec=... %F is honest?
//   4. What happens for a mixed file+directory selection?
//
// Build (Qt6 + KF6 headers; this is a probe, never a dependency of the
// shipped product — nothing in the workspace links Qt):
//
//   g++ -fPIC -std=c++20 servicemenu-match.cpp -o servicemenu-match \
//       $(pkg-config --cflags --libs Qt6Widgets) \
//       -I/usr/include/KF6/KIOCore -I/usr/include/KF6/KIOWidgets \
//       -I/usr/include/KF6/KIO -I/usr/include/KF6/KCoreAddons \
//       -I/usr/include/KF6/KService -I/usr/include/KF6 \
//       -lKF6KIOCore -lKF6KIOWidgets
//
// Run against a scratch XDG_DATA_HOME so the measurement never depends on,
// or disturbs, the session's own servicemenus:
//
//   XDG_DATA_HOME=/tmp/d XDG_CACHE_HOME=/tmp/c QT_QPA_PLATFORM=offscreen \
//       ./servicemenu-match /tmp/d/subject.bin
//
// Safety: reads no file contents — KFileItem is constructed from a URL and
// only the mimetype is asked for. That matters here more than usual: on a
// hydration mount, reading a placeholder's content is what hydrates it, so a
// probe that opened its subject would change the thing it measured.
#include <QAction>
#include <QApplication>
#include <QMenu>
#include <QUrl>
#include <KFileItem>
#include <KFileItemActions>
#include <KFileItemListProperties>
#include <cstdio>

int main(int argc, char **argv)
{
    QApplication app(argc, argv);
    if (argc < 2) {
        fprintf(stderr, "usage: servicemenu-match <path> [<path>...]\n");
        return 2;
    }

    KFileItemList list;
    for (int i = 1; i < argc; ++i) {
        KFileItem item(QUrl::fromLocalFile(QString::fromLocal8Bit(argv[i])));
        list << item;
        printf("input: %s  mimetype: %s  isdir: %d\n",
               argv[i], qPrintable(item.mimetype()), item.isDir());
    }

    KFileItemActions actions;
    actions.setItemListProperties(KFileItemListProperties(list));
    QMenu menu;
    // Services only: plugins are the compiled KFileItemActionPlugins already
    // installed on the machine, and counting them would make the answer
    // depend on what else is installed rather than on the entry under test.
    actions.addActionsTo(&menu, KFileItemActions::MenuActionSource::Services);

    int n = 0;
    for (QAction *a : menu.actions()) {
        if (a->isSeparator()) {
            continue;
        }
        printf("ACTION: %s\n", qPrintable(a->text()));
        n++;
        if (a->menu()) {
            for (QAction *sub : a->menu()->actions()) {
                if (!sub->isSeparator()) {
                    printf("  SUB: %s\n", qPrintable(sub->text()));
                    n++;
                }
            }
        }
    }
    printf("total: %d\n", n);
    return 0;
}
