// Local persistence.
//
// Two things are kept: desktops this device has joined before, and the
// performer's own monitor settings.
//
// **The join token is never stored.** It is a short-lived bearer credential
// and `docs/protocol/pairing.md` obligation 9 is explicit about it: what
// identifies this device afterwards is its long-term key, not the code it
// arrived with. A token on disk would be a credential outliving its purpose
// for no benefit.

import 'dart:io' show File;

import 'package:drift/drift.dart';
import 'package:drift/native.dart';
import 'package:path/path.dart' as p;
import 'package:path_provider/path_provider.dart';
import 'package:sqlite3_flutter_libs/sqlite3_flutter_libs.dart';

part 'database.g.dart';

/// A desktop this device has joined.
@DataClassName('TrustedDesktop')
class TrustedDesktops extends Table {
  /// The desktop's key fingerprint, which is what identifies it.
  ///
  /// Not the address: a laptop's IP changes between venues and even between
  /// sessions, and treating the address as identity would make every DHCP
  /// lease look like a different desktop.
  TextColumn get fingerprint => text()();

  /// Last known address, for an informed retry before falling back to
  /// scanning a new code.
  TextColumn get host => text()();
  IntColumn get controlPort => integer()();
  IntColumn get streamPort => integer()();

  /// What the operator called it, if the welcome said.
  TextColumn get name => text().nullable()();

  DateTimeColumn get firstJoinedAt => dateTime()();
  DateTimeColumn get lastJoinedAt => dateTime()();

  @override
  Set<Column> get primaryKey => {fingerprint};
}

/// The performer's own monitor settings.
///
/// One row, because a performer has one pair of ears. Named presets are a
/// later addition; this is what the app restores on launch so somebody does
/// not have to set their volume again every service.
@DataClassName('MonitorPreference')
class MonitorPreferences extends Table {
  IntColumn get id => integer().withDefault(const Constant(0))();

  RealColumn get gainDb => real().withDefault(const Constant(-6))();
  BoolColumn get muted => boolean().withDefault(const Constant(false))();
  RealColumn get lowDb => real().withDefault(const Constant(0))();
  RealColumn get midDb => real().withDefault(const Constant(0))();
  RealColumn get highDb => real().withDefault(const Constant(0))();

  /// 0 tight, 1 normal, 2 safe.
  IntColumn get bufferPreset => integer().withDefault(const Constant(1))();

  /// 0 protected, 1 standard, 2 unrestricted.
  ///
  /// Persisted, but deliberately *not* restored above protected without the
  /// performer choosing it again in that session — see `MonitorRepository`.
  IntColumn get safetyProfile => integer().withDefault(const Constant(0))();

  @override
  Set<Column> get primaryKey => {id};
}

@DriftDatabase(tables: [TrustedDesktops, MonitorPreferences])
class StageLinkDatabase extends _$StageLinkDatabase {
  StageLinkDatabase() : super(_open());

  /// For tests: an in-memory database with no file and no platform plugins.
  StageLinkDatabase.memory() : super(NativeDatabase.memory());

  @override
  int get schemaVersion => 1;

  static QueryExecutor _open() {
    return LazyDatabase(() async {
      final directory = await getApplicationDocumentsDirectory();
      final file = p.join(directory.path, 'stagelink.sqlite');
      // Some older Android builds ship an sqlite3 that drift cannot use; this
      // makes the bundled one win. Harmless on a device that does not need it.
      await applyWorkaroundToOpenSqlite3OnOldAndroidVersions();
      return NativeDatabase.createInBackground(File(file));
    });
  }
}
