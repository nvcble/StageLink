// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'database.dart';

// ignore_for_file: type=lint
class $TrustedDesktopsTable extends TrustedDesktops
    with TableInfo<$TrustedDesktopsTable, TrustedDesktop> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $TrustedDesktopsTable(this.attachedDatabase, [this._alias]);
  static const VerificationMeta _fingerprintMeta = const VerificationMeta(
    'fingerprint',
  );
  @override
  late final GeneratedColumn<String> fingerprint = GeneratedColumn<String>(
    'fingerprint',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _hostMeta = const VerificationMeta('host');
  @override
  late final GeneratedColumn<String> host = GeneratedColumn<String>(
    'host',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _controlPortMeta = const VerificationMeta(
    'controlPort',
  );
  @override
  late final GeneratedColumn<int> controlPort = GeneratedColumn<int>(
    'control_port',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _streamPortMeta = const VerificationMeta(
    'streamPort',
  );
  @override
  late final GeneratedColumn<int> streamPort = GeneratedColumn<int>(
    'stream_port',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _nameMeta = const VerificationMeta('name');
  @override
  late final GeneratedColumn<String> name = GeneratedColumn<String>(
    'name',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _firstJoinedAtMeta = const VerificationMeta(
    'firstJoinedAt',
  );
  @override
  late final GeneratedColumn<DateTime> firstJoinedAt =
      GeneratedColumn<DateTime>(
        'first_joined_at',
        aliasedName,
        false,
        type: DriftSqlType.dateTime,
        requiredDuringInsert: true,
      );
  static const VerificationMeta _lastJoinedAtMeta = const VerificationMeta(
    'lastJoinedAt',
  );
  @override
  late final GeneratedColumn<DateTime> lastJoinedAt = GeneratedColumn<DateTime>(
    'last_joined_at',
    aliasedName,
    false,
    type: DriftSqlType.dateTime,
    requiredDuringInsert: true,
  );
  @override
  List<GeneratedColumn> get $columns => [
    fingerprint,
    host,
    controlPort,
    streamPort,
    name,
    firstJoinedAt,
    lastJoinedAt,
  ];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'trusted_desktops';
  @override
  VerificationContext validateIntegrity(
    Insertable<TrustedDesktop> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('fingerprint')) {
      context.handle(
        _fingerprintMeta,
        fingerprint.isAcceptableOrUnknown(
          data['fingerprint']!,
          _fingerprintMeta,
        ),
      );
    } else if (isInserting) {
      context.missing(_fingerprintMeta);
    }
    if (data.containsKey('host')) {
      context.handle(
        _hostMeta,
        host.isAcceptableOrUnknown(data['host']!, _hostMeta),
      );
    } else if (isInserting) {
      context.missing(_hostMeta);
    }
    if (data.containsKey('control_port')) {
      context.handle(
        _controlPortMeta,
        controlPort.isAcceptableOrUnknown(
          data['control_port']!,
          _controlPortMeta,
        ),
      );
    } else if (isInserting) {
      context.missing(_controlPortMeta);
    }
    if (data.containsKey('stream_port')) {
      context.handle(
        _streamPortMeta,
        streamPort.isAcceptableOrUnknown(data['stream_port']!, _streamPortMeta),
      );
    } else if (isInserting) {
      context.missing(_streamPortMeta);
    }
    if (data.containsKey('name')) {
      context.handle(
        _nameMeta,
        name.isAcceptableOrUnknown(data['name']!, _nameMeta),
      );
    }
    if (data.containsKey('first_joined_at')) {
      context.handle(
        _firstJoinedAtMeta,
        firstJoinedAt.isAcceptableOrUnknown(
          data['first_joined_at']!,
          _firstJoinedAtMeta,
        ),
      );
    } else if (isInserting) {
      context.missing(_firstJoinedAtMeta);
    }
    if (data.containsKey('last_joined_at')) {
      context.handle(
        _lastJoinedAtMeta,
        lastJoinedAt.isAcceptableOrUnknown(
          data['last_joined_at']!,
          _lastJoinedAtMeta,
        ),
      );
    } else if (isInserting) {
      context.missing(_lastJoinedAtMeta);
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {fingerprint};
  @override
  TrustedDesktop map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return TrustedDesktop(
      fingerprint: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}fingerprint'],
      )!,
      host: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}host'],
      )!,
      controlPort: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}control_port'],
      )!,
      streamPort: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}stream_port'],
      )!,
      name: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}name'],
      ),
      firstJoinedAt: attachedDatabase.typeMapping.read(
        DriftSqlType.dateTime,
        data['${effectivePrefix}first_joined_at'],
      )!,
      lastJoinedAt: attachedDatabase.typeMapping.read(
        DriftSqlType.dateTime,
        data['${effectivePrefix}last_joined_at'],
      )!,
    );
  }

  @override
  $TrustedDesktopsTable createAlias(String alias) {
    return $TrustedDesktopsTable(attachedDatabase, alias);
  }
}

class TrustedDesktop extends DataClass implements Insertable<TrustedDesktop> {
  /// The desktop's key fingerprint, which is what identifies it.
  ///
  /// Not the address: a laptop's IP changes between venues and even between
  /// sessions, and treating the address as identity would make every DHCP
  /// lease look like a different desktop.
  final String fingerprint;

  /// Last known address, for an informed retry before falling back to
  /// scanning a new code.
  final String host;
  final int controlPort;
  final int streamPort;

  /// What the operator called it, if the welcome said.
  final String? name;
  final DateTime firstJoinedAt;
  final DateTime lastJoinedAt;
  const TrustedDesktop({
    required this.fingerprint,
    required this.host,
    required this.controlPort,
    required this.streamPort,
    this.name,
    required this.firstJoinedAt,
    required this.lastJoinedAt,
  });
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['fingerprint'] = Variable<String>(fingerprint);
    map['host'] = Variable<String>(host);
    map['control_port'] = Variable<int>(controlPort);
    map['stream_port'] = Variable<int>(streamPort);
    if (!nullToAbsent || name != null) {
      map['name'] = Variable<String>(name);
    }
    map['first_joined_at'] = Variable<DateTime>(firstJoinedAt);
    map['last_joined_at'] = Variable<DateTime>(lastJoinedAt);
    return map;
  }

  TrustedDesktopsCompanion toCompanion(bool nullToAbsent) {
    return TrustedDesktopsCompanion(
      fingerprint: Value(fingerprint),
      host: Value(host),
      controlPort: Value(controlPort),
      streamPort: Value(streamPort),
      name: name == null && nullToAbsent ? const Value.absent() : Value(name),
      firstJoinedAt: Value(firstJoinedAt),
      lastJoinedAt: Value(lastJoinedAt),
    );
  }

  factory TrustedDesktop.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return TrustedDesktop(
      fingerprint: serializer.fromJson<String>(json['fingerprint']),
      host: serializer.fromJson<String>(json['host']),
      controlPort: serializer.fromJson<int>(json['controlPort']),
      streamPort: serializer.fromJson<int>(json['streamPort']),
      name: serializer.fromJson<String?>(json['name']),
      firstJoinedAt: serializer.fromJson<DateTime>(json['firstJoinedAt']),
      lastJoinedAt: serializer.fromJson<DateTime>(json['lastJoinedAt']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'fingerprint': serializer.toJson<String>(fingerprint),
      'host': serializer.toJson<String>(host),
      'controlPort': serializer.toJson<int>(controlPort),
      'streamPort': serializer.toJson<int>(streamPort),
      'name': serializer.toJson<String?>(name),
      'firstJoinedAt': serializer.toJson<DateTime>(firstJoinedAt),
      'lastJoinedAt': serializer.toJson<DateTime>(lastJoinedAt),
    };
  }

  TrustedDesktop copyWith({
    String? fingerprint,
    String? host,
    int? controlPort,
    int? streamPort,
    Value<String?> name = const Value.absent(),
    DateTime? firstJoinedAt,
    DateTime? lastJoinedAt,
  }) => TrustedDesktop(
    fingerprint: fingerprint ?? this.fingerprint,
    host: host ?? this.host,
    controlPort: controlPort ?? this.controlPort,
    streamPort: streamPort ?? this.streamPort,
    name: name.present ? name.value : this.name,
    firstJoinedAt: firstJoinedAt ?? this.firstJoinedAt,
    lastJoinedAt: lastJoinedAt ?? this.lastJoinedAt,
  );
  TrustedDesktop copyWithCompanion(TrustedDesktopsCompanion data) {
    return TrustedDesktop(
      fingerprint: data.fingerprint.present
          ? data.fingerprint.value
          : this.fingerprint,
      host: data.host.present ? data.host.value : this.host,
      controlPort: data.controlPort.present
          ? data.controlPort.value
          : this.controlPort,
      streamPort: data.streamPort.present
          ? data.streamPort.value
          : this.streamPort,
      name: data.name.present ? data.name.value : this.name,
      firstJoinedAt: data.firstJoinedAt.present
          ? data.firstJoinedAt.value
          : this.firstJoinedAt,
      lastJoinedAt: data.lastJoinedAt.present
          ? data.lastJoinedAt.value
          : this.lastJoinedAt,
    );
  }

  @override
  String toString() {
    return (StringBuffer('TrustedDesktop(')
          ..write('fingerprint: $fingerprint, ')
          ..write('host: $host, ')
          ..write('controlPort: $controlPort, ')
          ..write('streamPort: $streamPort, ')
          ..write('name: $name, ')
          ..write('firstJoinedAt: $firstJoinedAt, ')
          ..write('lastJoinedAt: $lastJoinedAt')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode => Object.hash(
    fingerprint,
    host,
    controlPort,
    streamPort,
    name,
    firstJoinedAt,
    lastJoinedAt,
  );
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is TrustedDesktop &&
          other.fingerprint == this.fingerprint &&
          other.host == this.host &&
          other.controlPort == this.controlPort &&
          other.streamPort == this.streamPort &&
          other.name == this.name &&
          other.firstJoinedAt == this.firstJoinedAt &&
          other.lastJoinedAt == this.lastJoinedAt);
}

class TrustedDesktopsCompanion extends UpdateCompanion<TrustedDesktop> {
  final Value<String> fingerprint;
  final Value<String> host;
  final Value<int> controlPort;
  final Value<int> streamPort;
  final Value<String?> name;
  final Value<DateTime> firstJoinedAt;
  final Value<DateTime> lastJoinedAt;
  final Value<int> rowid;
  const TrustedDesktopsCompanion({
    this.fingerprint = const Value.absent(),
    this.host = const Value.absent(),
    this.controlPort = const Value.absent(),
    this.streamPort = const Value.absent(),
    this.name = const Value.absent(),
    this.firstJoinedAt = const Value.absent(),
    this.lastJoinedAt = const Value.absent(),
    this.rowid = const Value.absent(),
  });
  TrustedDesktopsCompanion.insert({
    required String fingerprint,
    required String host,
    required int controlPort,
    required int streamPort,
    this.name = const Value.absent(),
    required DateTime firstJoinedAt,
    required DateTime lastJoinedAt,
    this.rowid = const Value.absent(),
  }) : fingerprint = Value(fingerprint),
       host = Value(host),
       controlPort = Value(controlPort),
       streamPort = Value(streamPort),
       firstJoinedAt = Value(firstJoinedAt),
       lastJoinedAt = Value(lastJoinedAt);
  static Insertable<TrustedDesktop> custom({
    Expression<String>? fingerprint,
    Expression<String>? host,
    Expression<int>? controlPort,
    Expression<int>? streamPort,
    Expression<String>? name,
    Expression<DateTime>? firstJoinedAt,
    Expression<DateTime>? lastJoinedAt,
    Expression<int>? rowid,
  }) {
    return RawValuesInsertable({
      if (fingerprint != null) 'fingerprint': fingerprint,
      if (host != null) 'host': host,
      if (controlPort != null) 'control_port': controlPort,
      if (streamPort != null) 'stream_port': streamPort,
      if (name != null) 'name': name,
      if (firstJoinedAt != null) 'first_joined_at': firstJoinedAt,
      if (lastJoinedAt != null) 'last_joined_at': lastJoinedAt,
      if (rowid != null) 'rowid': rowid,
    });
  }

  TrustedDesktopsCompanion copyWith({
    Value<String>? fingerprint,
    Value<String>? host,
    Value<int>? controlPort,
    Value<int>? streamPort,
    Value<String?>? name,
    Value<DateTime>? firstJoinedAt,
    Value<DateTime>? lastJoinedAt,
    Value<int>? rowid,
  }) {
    return TrustedDesktopsCompanion(
      fingerprint: fingerprint ?? this.fingerprint,
      host: host ?? this.host,
      controlPort: controlPort ?? this.controlPort,
      streamPort: streamPort ?? this.streamPort,
      name: name ?? this.name,
      firstJoinedAt: firstJoinedAt ?? this.firstJoinedAt,
      lastJoinedAt: lastJoinedAt ?? this.lastJoinedAt,
      rowid: rowid ?? this.rowid,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (fingerprint.present) {
      map['fingerprint'] = Variable<String>(fingerprint.value);
    }
    if (host.present) {
      map['host'] = Variable<String>(host.value);
    }
    if (controlPort.present) {
      map['control_port'] = Variable<int>(controlPort.value);
    }
    if (streamPort.present) {
      map['stream_port'] = Variable<int>(streamPort.value);
    }
    if (name.present) {
      map['name'] = Variable<String>(name.value);
    }
    if (firstJoinedAt.present) {
      map['first_joined_at'] = Variable<DateTime>(firstJoinedAt.value);
    }
    if (lastJoinedAt.present) {
      map['last_joined_at'] = Variable<DateTime>(lastJoinedAt.value);
    }
    if (rowid.present) {
      map['rowid'] = Variable<int>(rowid.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('TrustedDesktopsCompanion(')
          ..write('fingerprint: $fingerprint, ')
          ..write('host: $host, ')
          ..write('controlPort: $controlPort, ')
          ..write('streamPort: $streamPort, ')
          ..write('name: $name, ')
          ..write('firstJoinedAt: $firstJoinedAt, ')
          ..write('lastJoinedAt: $lastJoinedAt, ')
          ..write('rowid: $rowid')
          ..write(')'))
        .toString();
  }
}

class $MonitorPreferencesTable extends MonitorPreferences
    with TableInfo<$MonitorPreferencesTable, MonitorPreference> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $MonitorPreferencesTable(this.attachedDatabase, [this._alias]);
  static const VerificationMeta _idMeta = const VerificationMeta('id');
  @override
  late final GeneratedColumn<int> id = GeneratedColumn<int>(
    'id',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
    defaultValue: const Constant(0),
  );
  static const VerificationMeta _gainDbMeta = const VerificationMeta('gainDb');
  @override
  late final GeneratedColumn<double> gainDb = GeneratedColumn<double>(
    'gain_db',
    aliasedName,
    false,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
    defaultValue: const Constant(-6),
  );
  static const VerificationMeta _mutedMeta = const VerificationMeta('muted');
  @override
  late final GeneratedColumn<bool> muted = GeneratedColumn<bool>(
    'muted',
    aliasedName,
    false,
    type: DriftSqlType.bool,
    requiredDuringInsert: false,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'CHECK ("muted" IN (0, 1))',
    ),
    defaultValue: const Constant(false),
  );
  static const VerificationMeta _lowDbMeta = const VerificationMeta('lowDb');
  @override
  late final GeneratedColumn<double> lowDb = GeneratedColumn<double>(
    'low_db',
    aliasedName,
    false,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
    defaultValue: const Constant(0),
  );
  static const VerificationMeta _midDbMeta = const VerificationMeta('midDb');
  @override
  late final GeneratedColumn<double> midDb = GeneratedColumn<double>(
    'mid_db',
    aliasedName,
    false,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
    defaultValue: const Constant(0),
  );
  static const VerificationMeta _highDbMeta = const VerificationMeta('highDb');
  @override
  late final GeneratedColumn<double> highDb = GeneratedColumn<double>(
    'high_db',
    aliasedName,
    false,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
    defaultValue: const Constant(0),
  );
  static const VerificationMeta _bufferPresetMeta = const VerificationMeta(
    'bufferPreset',
  );
  @override
  late final GeneratedColumn<int> bufferPreset = GeneratedColumn<int>(
    'buffer_preset',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
    defaultValue: const Constant(1),
  );
  static const VerificationMeta _safetyProfileMeta = const VerificationMeta(
    'safetyProfile',
  );
  @override
  late final GeneratedColumn<int> safetyProfile = GeneratedColumn<int>(
    'safety_profile',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
    defaultValue: const Constant(0),
  );
  @override
  List<GeneratedColumn> get $columns => [
    id,
    gainDb,
    muted,
    lowDb,
    midDb,
    highDb,
    bufferPreset,
    safetyProfile,
  ];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'monitor_preferences';
  @override
  VerificationContext validateIntegrity(
    Insertable<MonitorPreference> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('id')) {
      context.handle(_idMeta, id.isAcceptableOrUnknown(data['id']!, _idMeta));
    }
    if (data.containsKey('gain_db')) {
      context.handle(
        _gainDbMeta,
        gainDb.isAcceptableOrUnknown(data['gain_db']!, _gainDbMeta),
      );
    }
    if (data.containsKey('muted')) {
      context.handle(
        _mutedMeta,
        muted.isAcceptableOrUnknown(data['muted']!, _mutedMeta),
      );
    }
    if (data.containsKey('low_db')) {
      context.handle(
        _lowDbMeta,
        lowDb.isAcceptableOrUnknown(data['low_db']!, _lowDbMeta),
      );
    }
    if (data.containsKey('mid_db')) {
      context.handle(
        _midDbMeta,
        midDb.isAcceptableOrUnknown(data['mid_db']!, _midDbMeta),
      );
    }
    if (data.containsKey('high_db')) {
      context.handle(
        _highDbMeta,
        highDb.isAcceptableOrUnknown(data['high_db']!, _highDbMeta),
      );
    }
    if (data.containsKey('buffer_preset')) {
      context.handle(
        _bufferPresetMeta,
        bufferPreset.isAcceptableOrUnknown(
          data['buffer_preset']!,
          _bufferPresetMeta,
        ),
      );
    }
    if (data.containsKey('safety_profile')) {
      context.handle(
        _safetyProfileMeta,
        safetyProfile.isAcceptableOrUnknown(
          data['safety_profile']!,
          _safetyProfileMeta,
        ),
      );
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {id};
  @override
  MonitorPreference map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return MonitorPreference(
      id: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}id'],
      )!,
      gainDb: attachedDatabase.typeMapping.read(
        DriftSqlType.double,
        data['${effectivePrefix}gain_db'],
      )!,
      muted: attachedDatabase.typeMapping.read(
        DriftSqlType.bool,
        data['${effectivePrefix}muted'],
      )!,
      lowDb: attachedDatabase.typeMapping.read(
        DriftSqlType.double,
        data['${effectivePrefix}low_db'],
      )!,
      midDb: attachedDatabase.typeMapping.read(
        DriftSqlType.double,
        data['${effectivePrefix}mid_db'],
      )!,
      highDb: attachedDatabase.typeMapping.read(
        DriftSqlType.double,
        data['${effectivePrefix}high_db'],
      )!,
      bufferPreset: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}buffer_preset'],
      )!,
      safetyProfile: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}safety_profile'],
      )!,
    );
  }

  @override
  $MonitorPreferencesTable createAlias(String alias) {
    return $MonitorPreferencesTable(attachedDatabase, alias);
  }
}

class MonitorPreference extends DataClass
    implements Insertable<MonitorPreference> {
  final int id;
  final double gainDb;
  final bool muted;
  final double lowDb;
  final double midDb;
  final double highDb;

  /// 0 tight, 1 normal, 2 safe.
  final int bufferPreset;

  /// 0 protected, 1 standard, 2 unrestricted.
  ///
  /// Persisted, but deliberately *not* restored above protected without the
  /// performer choosing it again in that session — see `MonitorRepository`.
  final int safetyProfile;
  const MonitorPreference({
    required this.id,
    required this.gainDb,
    required this.muted,
    required this.lowDb,
    required this.midDb,
    required this.highDb,
    required this.bufferPreset,
    required this.safetyProfile,
  });
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['id'] = Variable<int>(id);
    map['gain_db'] = Variable<double>(gainDb);
    map['muted'] = Variable<bool>(muted);
    map['low_db'] = Variable<double>(lowDb);
    map['mid_db'] = Variable<double>(midDb);
    map['high_db'] = Variable<double>(highDb);
    map['buffer_preset'] = Variable<int>(bufferPreset);
    map['safety_profile'] = Variable<int>(safetyProfile);
    return map;
  }

  MonitorPreferencesCompanion toCompanion(bool nullToAbsent) {
    return MonitorPreferencesCompanion(
      id: Value(id),
      gainDb: Value(gainDb),
      muted: Value(muted),
      lowDb: Value(lowDb),
      midDb: Value(midDb),
      highDb: Value(highDb),
      bufferPreset: Value(bufferPreset),
      safetyProfile: Value(safetyProfile),
    );
  }

  factory MonitorPreference.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return MonitorPreference(
      id: serializer.fromJson<int>(json['id']),
      gainDb: serializer.fromJson<double>(json['gainDb']),
      muted: serializer.fromJson<bool>(json['muted']),
      lowDb: serializer.fromJson<double>(json['lowDb']),
      midDb: serializer.fromJson<double>(json['midDb']),
      highDb: serializer.fromJson<double>(json['highDb']),
      bufferPreset: serializer.fromJson<int>(json['bufferPreset']),
      safetyProfile: serializer.fromJson<int>(json['safetyProfile']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'id': serializer.toJson<int>(id),
      'gainDb': serializer.toJson<double>(gainDb),
      'muted': serializer.toJson<bool>(muted),
      'lowDb': serializer.toJson<double>(lowDb),
      'midDb': serializer.toJson<double>(midDb),
      'highDb': serializer.toJson<double>(highDb),
      'bufferPreset': serializer.toJson<int>(bufferPreset),
      'safetyProfile': serializer.toJson<int>(safetyProfile),
    };
  }

  MonitorPreference copyWith({
    int? id,
    double? gainDb,
    bool? muted,
    double? lowDb,
    double? midDb,
    double? highDb,
    int? bufferPreset,
    int? safetyProfile,
  }) => MonitorPreference(
    id: id ?? this.id,
    gainDb: gainDb ?? this.gainDb,
    muted: muted ?? this.muted,
    lowDb: lowDb ?? this.lowDb,
    midDb: midDb ?? this.midDb,
    highDb: highDb ?? this.highDb,
    bufferPreset: bufferPreset ?? this.bufferPreset,
    safetyProfile: safetyProfile ?? this.safetyProfile,
  );
  MonitorPreference copyWithCompanion(MonitorPreferencesCompanion data) {
    return MonitorPreference(
      id: data.id.present ? data.id.value : this.id,
      gainDb: data.gainDb.present ? data.gainDb.value : this.gainDb,
      muted: data.muted.present ? data.muted.value : this.muted,
      lowDb: data.lowDb.present ? data.lowDb.value : this.lowDb,
      midDb: data.midDb.present ? data.midDb.value : this.midDb,
      highDb: data.highDb.present ? data.highDb.value : this.highDb,
      bufferPreset: data.bufferPreset.present
          ? data.bufferPreset.value
          : this.bufferPreset,
      safetyProfile: data.safetyProfile.present
          ? data.safetyProfile.value
          : this.safetyProfile,
    );
  }

  @override
  String toString() {
    return (StringBuffer('MonitorPreference(')
          ..write('id: $id, ')
          ..write('gainDb: $gainDb, ')
          ..write('muted: $muted, ')
          ..write('lowDb: $lowDb, ')
          ..write('midDb: $midDb, ')
          ..write('highDb: $highDb, ')
          ..write('bufferPreset: $bufferPreset, ')
          ..write('safetyProfile: $safetyProfile')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode => Object.hash(
    id,
    gainDb,
    muted,
    lowDb,
    midDb,
    highDb,
    bufferPreset,
    safetyProfile,
  );
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is MonitorPreference &&
          other.id == this.id &&
          other.gainDb == this.gainDb &&
          other.muted == this.muted &&
          other.lowDb == this.lowDb &&
          other.midDb == this.midDb &&
          other.highDb == this.highDb &&
          other.bufferPreset == this.bufferPreset &&
          other.safetyProfile == this.safetyProfile);
}

class MonitorPreferencesCompanion extends UpdateCompanion<MonitorPreference> {
  final Value<int> id;
  final Value<double> gainDb;
  final Value<bool> muted;
  final Value<double> lowDb;
  final Value<double> midDb;
  final Value<double> highDb;
  final Value<int> bufferPreset;
  final Value<int> safetyProfile;
  const MonitorPreferencesCompanion({
    this.id = const Value.absent(),
    this.gainDb = const Value.absent(),
    this.muted = const Value.absent(),
    this.lowDb = const Value.absent(),
    this.midDb = const Value.absent(),
    this.highDb = const Value.absent(),
    this.bufferPreset = const Value.absent(),
    this.safetyProfile = const Value.absent(),
  });
  MonitorPreferencesCompanion.insert({
    this.id = const Value.absent(),
    this.gainDb = const Value.absent(),
    this.muted = const Value.absent(),
    this.lowDb = const Value.absent(),
    this.midDb = const Value.absent(),
    this.highDb = const Value.absent(),
    this.bufferPreset = const Value.absent(),
    this.safetyProfile = const Value.absent(),
  });
  static Insertable<MonitorPreference> custom({
    Expression<int>? id,
    Expression<double>? gainDb,
    Expression<bool>? muted,
    Expression<double>? lowDb,
    Expression<double>? midDb,
    Expression<double>? highDb,
    Expression<int>? bufferPreset,
    Expression<int>? safetyProfile,
  }) {
    return RawValuesInsertable({
      if (id != null) 'id': id,
      if (gainDb != null) 'gain_db': gainDb,
      if (muted != null) 'muted': muted,
      if (lowDb != null) 'low_db': lowDb,
      if (midDb != null) 'mid_db': midDb,
      if (highDb != null) 'high_db': highDb,
      if (bufferPreset != null) 'buffer_preset': bufferPreset,
      if (safetyProfile != null) 'safety_profile': safetyProfile,
    });
  }

  MonitorPreferencesCompanion copyWith({
    Value<int>? id,
    Value<double>? gainDb,
    Value<bool>? muted,
    Value<double>? lowDb,
    Value<double>? midDb,
    Value<double>? highDb,
    Value<int>? bufferPreset,
    Value<int>? safetyProfile,
  }) {
    return MonitorPreferencesCompanion(
      id: id ?? this.id,
      gainDb: gainDb ?? this.gainDb,
      muted: muted ?? this.muted,
      lowDb: lowDb ?? this.lowDb,
      midDb: midDb ?? this.midDb,
      highDb: highDb ?? this.highDb,
      bufferPreset: bufferPreset ?? this.bufferPreset,
      safetyProfile: safetyProfile ?? this.safetyProfile,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (id.present) {
      map['id'] = Variable<int>(id.value);
    }
    if (gainDb.present) {
      map['gain_db'] = Variable<double>(gainDb.value);
    }
    if (muted.present) {
      map['muted'] = Variable<bool>(muted.value);
    }
    if (lowDb.present) {
      map['low_db'] = Variable<double>(lowDb.value);
    }
    if (midDb.present) {
      map['mid_db'] = Variable<double>(midDb.value);
    }
    if (highDb.present) {
      map['high_db'] = Variable<double>(highDb.value);
    }
    if (bufferPreset.present) {
      map['buffer_preset'] = Variable<int>(bufferPreset.value);
    }
    if (safetyProfile.present) {
      map['safety_profile'] = Variable<int>(safetyProfile.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('MonitorPreferencesCompanion(')
          ..write('id: $id, ')
          ..write('gainDb: $gainDb, ')
          ..write('muted: $muted, ')
          ..write('lowDb: $lowDb, ')
          ..write('midDb: $midDb, ')
          ..write('highDb: $highDb, ')
          ..write('bufferPreset: $bufferPreset, ')
          ..write('safetyProfile: $safetyProfile')
          ..write(')'))
        .toString();
  }
}

abstract class _$StageLinkDatabase extends GeneratedDatabase {
  _$StageLinkDatabase(QueryExecutor e) : super(e);
  $StageLinkDatabaseManager get managers => $StageLinkDatabaseManager(this);
  late final $TrustedDesktopsTable trustedDesktops = $TrustedDesktopsTable(
    this,
  );
  late final $MonitorPreferencesTable monitorPreferences =
      $MonitorPreferencesTable(this);
  @override
  Iterable<TableInfo<Table, Object?>> get allTables =>
      allSchemaEntities.whereType<TableInfo<Table, Object?>>();
  @override
  List<DatabaseSchemaEntity> get allSchemaEntities => [
    trustedDesktops,
    monitorPreferences,
  ];
}

typedef $$TrustedDesktopsTableCreateCompanionBuilder =
    TrustedDesktopsCompanion Function({
      required String fingerprint,
      required String host,
      required int controlPort,
      required int streamPort,
      Value<String?> name,
      required DateTime firstJoinedAt,
      required DateTime lastJoinedAt,
      Value<int> rowid,
    });
typedef $$TrustedDesktopsTableUpdateCompanionBuilder =
    TrustedDesktopsCompanion Function({
      Value<String> fingerprint,
      Value<String> host,
      Value<int> controlPort,
      Value<int> streamPort,
      Value<String?> name,
      Value<DateTime> firstJoinedAt,
      Value<DateTime> lastJoinedAt,
      Value<int> rowid,
    });

class $$TrustedDesktopsTableFilterComposer
    extends Composer<_$StageLinkDatabase, $TrustedDesktopsTable> {
  $$TrustedDesktopsTableFilterComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnFilters<String> get fingerprint => $composableBuilder(
    column: $table.fingerprint,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get host => $composableBuilder(
    column: $table.host,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get controlPort => $composableBuilder(
    column: $table.controlPort,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get streamPort => $composableBuilder(
    column: $table.streamPort,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get name => $composableBuilder(
    column: $table.name,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<DateTime> get firstJoinedAt => $composableBuilder(
    column: $table.firstJoinedAt,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<DateTime> get lastJoinedAt => $composableBuilder(
    column: $table.lastJoinedAt,
    builder: (column) => ColumnFilters(column),
  );
}

class $$TrustedDesktopsTableOrderingComposer
    extends Composer<_$StageLinkDatabase, $TrustedDesktopsTable> {
  $$TrustedDesktopsTableOrderingComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnOrderings<String> get fingerprint => $composableBuilder(
    column: $table.fingerprint,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get host => $composableBuilder(
    column: $table.host,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get controlPort => $composableBuilder(
    column: $table.controlPort,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get streamPort => $composableBuilder(
    column: $table.streamPort,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get name => $composableBuilder(
    column: $table.name,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<DateTime> get firstJoinedAt => $composableBuilder(
    column: $table.firstJoinedAt,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<DateTime> get lastJoinedAt => $composableBuilder(
    column: $table.lastJoinedAt,
    builder: (column) => ColumnOrderings(column),
  );
}

class $$TrustedDesktopsTableAnnotationComposer
    extends Composer<_$StageLinkDatabase, $TrustedDesktopsTable> {
  $$TrustedDesktopsTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumn<String> get fingerprint => $composableBuilder(
    column: $table.fingerprint,
    builder: (column) => column,
  );

  GeneratedColumn<String> get host =>
      $composableBuilder(column: $table.host, builder: (column) => column);

  GeneratedColumn<int> get controlPort => $composableBuilder(
    column: $table.controlPort,
    builder: (column) => column,
  );

  GeneratedColumn<int> get streamPort => $composableBuilder(
    column: $table.streamPort,
    builder: (column) => column,
  );

  GeneratedColumn<String> get name =>
      $composableBuilder(column: $table.name, builder: (column) => column);

  GeneratedColumn<DateTime> get firstJoinedAt => $composableBuilder(
    column: $table.firstJoinedAt,
    builder: (column) => column,
  );

  GeneratedColumn<DateTime> get lastJoinedAt => $composableBuilder(
    column: $table.lastJoinedAt,
    builder: (column) => column,
  );
}

class $$TrustedDesktopsTableTableManager
    extends
        RootTableManager<
          _$StageLinkDatabase,
          $TrustedDesktopsTable,
          TrustedDesktop,
          $$TrustedDesktopsTableFilterComposer,
          $$TrustedDesktopsTableOrderingComposer,
          $$TrustedDesktopsTableAnnotationComposer,
          $$TrustedDesktopsTableCreateCompanionBuilder,
          $$TrustedDesktopsTableUpdateCompanionBuilder,
          (
            TrustedDesktop,
            BaseReferences<
              _$StageLinkDatabase,
              $TrustedDesktopsTable,
              TrustedDesktop
            >,
          ),
          TrustedDesktop,
          PrefetchHooks Function()
        > {
  $$TrustedDesktopsTableTableManager(
    _$StageLinkDatabase db,
    $TrustedDesktopsTable table,
  ) : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$TrustedDesktopsTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$TrustedDesktopsTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$TrustedDesktopsTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback:
              ({
                Value<String> fingerprint = const Value.absent(),
                Value<String> host = const Value.absent(),
                Value<int> controlPort = const Value.absent(),
                Value<int> streamPort = const Value.absent(),
                Value<String?> name = const Value.absent(),
                Value<DateTime> firstJoinedAt = const Value.absent(),
                Value<DateTime> lastJoinedAt = const Value.absent(),
                Value<int> rowid = const Value.absent(),
              }) => TrustedDesktopsCompanion(
                fingerprint: fingerprint,
                host: host,
                controlPort: controlPort,
                streamPort: streamPort,
                name: name,
                firstJoinedAt: firstJoinedAt,
                lastJoinedAt: lastJoinedAt,
                rowid: rowid,
              ),
          createCompanionCallback:
              ({
                required String fingerprint,
                required String host,
                required int controlPort,
                required int streamPort,
                Value<String?> name = const Value.absent(),
                required DateTime firstJoinedAt,
                required DateTime lastJoinedAt,
                Value<int> rowid = const Value.absent(),
              }) => TrustedDesktopsCompanion.insert(
                fingerprint: fingerprint,
                host: host,
                controlPort: controlPort,
                streamPort: streamPort,
                name: name,
                firstJoinedAt: firstJoinedAt,
                lastJoinedAt: lastJoinedAt,
                rowid: rowid,
              ),
          withReferenceMapper: (p0) => p0
              .map((e) => (e.readTable(table), BaseReferences(db, table, e)))
              .toList(),
          prefetchHooksCallback: null,
        ),
      );
}

typedef $$TrustedDesktopsTableProcessedTableManager =
    ProcessedTableManager<
      _$StageLinkDatabase,
      $TrustedDesktopsTable,
      TrustedDesktop,
      $$TrustedDesktopsTableFilterComposer,
      $$TrustedDesktopsTableOrderingComposer,
      $$TrustedDesktopsTableAnnotationComposer,
      $$TrustedDesktopsTableCreateCompanionBuilder,
      $$TrustedDesktopsTableUpdateCompanionBuilder,
      (
        TrustedDesktop,
        BaseReferences<
          _$StageLinkDatabase,
          $TrustedDesktopsTable,
          TrustedDesktop
        >,
      ),
      TrustedDesktop,
      PrefetchHooks Function()
    >;
typedef $$MonitorPreferencesTableCreateCompanionBuilder =
    MonitorPreferencesCompanion Function({
      Value<int> id,
      Value<double> gainDb,
      Value<bool> muted,
      Value<double> lowDb,
      Value<double> midDb,
      Value<double> highDb,
      Value<int> bufferPreset,
      Value<int> safetyProfile,
    });
typedef $$MonitorPreferencesTableUpdateCompanionBuilder =
    MonitorPreferencesCompanion Function({
      Value<int> id,
      Value<double> gainDb,
      Value<bool> muted,
      Value<double> lowDb,
      Value<double> midDb,
      Value<double> highDb,
      Value<int> bufferPreset,
      Value<int> safetyProfile,
    });

class $$MonitorPreferencesTableFilterComposer
    extends Composer<_$StageLinkDatabase, $MonitorPreferencesTable> {
  $$MonitorPreferencesTableFilterComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnFilters<int> get id => $composableBuilder(
    column: $table.id,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<double> get gainDb => $composableBuilder(
    column: $table.gainDb,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<bool> get muted => $composableBuilder(
    column: $table.muted,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<double> get lowDb => $composableBuilder(
    column: $table.lowDb,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<double> get midDb => $composableBuilder(
    column: $table.midDb,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<double> get highDb => $composableBuilder(
    column: $table.highDb,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get bufferPreset => $composableBuilder(
    column: $table.bufferPreset,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get safetyProfile => $composableBuilder(
    column: $table.safetyProfile,
    builder: (column) => ColumnFilters(column),
  );
}

class $$MonitorPreferencesTableOrderingComposer
    extends Composer<_$StageLinkDatabase, $MonitorPreferencesTable> {
  $$MonitorPreferencesTableOrderingComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnOrderings<int> get id => $composableBuilder(
    column: $table.id,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<double> get gainDb => $composableBuilder(
    column: $table.gainDb,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<bool> get muted => $composableBuilder(
    column: $table.muted,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<double> get lowDb => $composableBuilder(
    column: $table.lowDb,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<double> get midDb => $composableBuilder(
    column: $table.midDb,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<double> get highDb => $composableBuilder(
    column: $table.highDb,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get bufferPreset => $composableBuilder(
    column: $table.bufferPreset,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get safetyProfile => $composableBuilder(
    column: $table.safetyProfile,
    builder: (column) => ColumnOrderings(column),
  );
}

class $$MonitorPreferencesTableAnnotationComposer
    extends Composer<_$StageLinkDatabase, $MonitorPreferencesTable> {
  $$MonitorPreferencesTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumn<int> get id =>
      $composableBuilder(column: $table.id, builder: (column) => column);

  GeneratedColumn<double> get gainDb =>
      $composableBuilder(column: $table.gainDb, builder: (column) => column);

  GeneratedColumn<bool> get muted =>
      $composableBuilder(column: $table.muted, builder: (column) => column);

  GeneratedColumn<double> get lowDb =>
      $composableBuilder(column: $table.lowDb, builder: (column) => column);

  GeneratedColumn<double> get midDb =>
      $composableBuilder(column: $table.midDb, builder: (column) => column);

  GeneratedColumn<double> get highDb =>
      $composableBuilder(column: $table.highDb, builder: (column) => column);

  GeneratedColumn<int> get bufferPreset => $composableBuilder(
    column: $table.bufferPreset,
    builder: (column) => column,
  );

  GeneratedColumn<int> get safetyProfile => $composableBuilder(
    column: $table.safetyProfile,
    builder: (column) => column,
  );
}

class $$MonitorPreferencesTableTableManager
    extends
        RootTableManager<
          _$StageLinkDatabase,
          $MonitorPreferencesTable,
          MonitorPreference,
          $$MonitorPreferencesTableFilterComposer,
          $$MonitorPreferencesTableOrderingComposer,
          $$MonitorPreferencesTableAnnotationComposer,
          $$MonitorPreferencesTableCreateCompanionBuilder,
          $$MonitorPreferencesTableUpdateCompanionBuilder,
          (
            MonitorPreference,
            BaseReferences<
              _$StageLinkDatabase,
              $MonitorPreferencesTable,
              MonitorPreference
            >,
          ),
          MonitorPreference,
          PrefetchHooks Function()
        > {
  $$MonitorPreferencesTableTableManager(
    _$StageLinkDatabase db,
    $MonitorPreferencesTable table,
  ) : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$MonitorPreferencesTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$MonitorPreferencesTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$MonitorPreferencesTableAnnotationComposer(
                $db: db,
                $table: table,
              ),
          updateCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                Value<double> gainDb = const Value.absent(),
                Value<bool> muted = const Value.absent(),
                Value<double> lowDb = const Value.absent(),
                Value<double> midDb = const Value.absent(),
                Value<double> highDb = const Value.absent(),
                Value<int> bufferPreset = const Value.absent(),
                Value<int> safetyProfile = const Value.absent(),
              }) => MonitorPreferencesCompanion(
                id: id,
                gainDb: gainDb,
                muted: muted,
                lowDb: lowDb,
                midDb: midDb,
                highDb: highDb,
                bufferPreset: bufferPreset,
                safetyProfile: safetyProfile,
              ),
          createCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                Value<double> gainDb = const Value.absent(),
                Value<bool> muted = const Value.absent(),
                Value<double> lowDb = const Value.absent(),
                Value<double> midDb = const Value.absent(),
                Value<double> highDb = const Value.absent(),
                Value<int> bufferPreset = const Value.absent(),
                Value<int> safetyProfile = const Value.absent(),
              }) => MonitorPreferencesCompanion.insert(
                id: id,
                gainDb: gainDb,
                muted: muted,
                lowDb: lowDb,
                midDb: midDb,
                highDb: highDb,
                bufferPreset: bufferPreset,
                safetyProfile: safetyProfile,
              ),
          withReferenceMapper: (p0) => p0
              .map((e) => (e.readTable(table), BaseReferences(db, table, e)))
              .toList(),
          prefetchHooksCallback: null,
        ),
      );
}

typedef $$MonitorPreferencesTableProcessedTableManager =
    ProcessedTableManager<
      _$StageLinkDatabase,
      $MonitorPreferencesTable,
      MonitorPreference,
      $$MonitorPreferencesTableFilterComposer,
      $$MonitorPreferencesTableOrderingComposer,
      $$MonitorPreferencesTableAnnotationComposer,
      $$MonitorPreferencesTableCreateCompanionBuilder,
      $$MonitorPreferencesTableUpdateCompanionBuilder,
      (
        MonitorPreference,
        BaseReferences<
          _$StageLinkDatabase,
          $MonitorPreferencesTable,
          MonitorPreference
        >,
      ),
      MonitorPreference,
      PrefetchHooks Function()
    >;

class $StageLinkDatabaseManager {
  final _$StageLinkDatabase _db;
  $StageLinkDatabaseManager(this._db);
  $$TrustedDesktopsTableTableManager get trustedDesktops =>
      $$TrustedDesktopsTableTableManager(_db, _db.trustedDesktops);
  $$MonitorPreferencesTableTableManager get monitorPreferences =>
      $$MonitorPreferencesTableTableManager(_db, _db.monitorPreferences);
}
