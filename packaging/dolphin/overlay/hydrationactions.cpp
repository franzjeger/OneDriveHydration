// SPDX-License-Identifier: MIT OR Apache-2.0
#include <KAbstractFileItemActionPlugin>
#include "selection.h"
#include <KFileItemListProperties>
#include <KPluginFactory>
#include <QAction>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QIcon>
#include <QMenu>
#include <QProcess>
#include <QStandardPaths>
#include <QTextStream>

class HydrationActions : public KAbstractFileItemActionPlugin
{
    Q_OBJECT
public:
    explicit HydrationActions(QObject *parent, const QVariantList &) : KAbstractFileItemActionPlugin(parent) {}

    QList<QAction *> actions(const KFileItemListProperties &items, QWidget *parent) override
    {
        QFile config(QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation)
            + QStringLiteral("/onedrive-hydration/overlay-roots"));
        if (!config.open(QIODevice::ReadOnly | QIODevice::Text)) return {};
        QStringList roots;
        QTextStream in(&config);
        while (!in.atEnd()) {
            const QString line = in.readLine().trimmed();
            if (!line.isEmpty() && !line.startsWith('#')) roots << QFileInfo(line).canonicalFilePath();
        }
        QStringList paths;
        for (const auto &url : items.urlList()) {
            if (!url.isLocalFile()) return {};
            const QString path = QFileInfo(url.toLocalFile()).canonicalFilePath();
            bool inside = false;
            for (const auto &root : roots)
                if (!root.isEmpty() && path.startsWith(root + '/') && selectionState(root, path) == SelectionState::Synced) inside = true;
            if (!inside) return {};
            paths << path;
        }
        if (paths.isEmpty()) return {};
        paths.removeDuplicates();
        auto *menu = new QMenu(parent);
        menu->setTitle(tr("OneDrive"));
        menu->setIcon(QIcon::fromTheme("onedrive-hydration"));
        const auto add = [&](const QString &label, const QString &icon, const QString &verb) {
            auto *action = menu->addAction(QIcon::fromTheme(icon), label);
            connect(action, &QAction::triggered, this, [this, paths, verb] {
                auto message = QDBusMessage::createMethodCall(QStringLiteral("io.github.franzjeger.OneDriveHydration"),
                    QStringLiteral("/io/github/franzjeger/OneDriveHydration"), QStringLiteral("io.github.franzjeger.OneDriveHydration"),
                    QStringLiteral("StartAvailabilityJob"));
                message << verb << paths;
                auto *call = new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(message), this);
                connect(call, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *done) {
                    if (done->reply().type() == QDBusMessage::ErrorMessage) Q_EMIT error(done->reply().errorMessage());
                    done->deleteLater();
                });
            });
        };
        add(tr("Keep on Device"), "onedrive-hydration-pinned", "keep");
        add(tr("Free Up Space"), "cloud-download", "free");
        return {menu->menuAction()};
    }
};

K_PLUGIN_CLASS_WITH_JSON(HydrationActions, "hydrationactions.json")
#include "hydrationactions.moc"
