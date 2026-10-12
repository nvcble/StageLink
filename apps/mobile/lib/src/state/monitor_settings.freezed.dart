// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'monitor_settings.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$MonitorSettings {

 double get gainDb; bool get muted; double get lowDb; double get midDb; double get highDb; BufferPreset get preset; SafetyProfile get safety;
/// Create a copy of MonitorSettings
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$MonitorSettingsCopyWith<MonitorSettings> get copyWith => _$MonitorSettingsCopyWithImpl<MonitorSettings>(this as MonitorSettings, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is MonitorSettings&&(identical(other.gainDb, gainDb) || other.gainDb == gainDb)&&(identical(other.muted, muted) || other.muted == muted)&&(identical(other.lowDb, lowDb) || other.lowDb == lowDb)&&(identical(other.midDb, midDb) || other.midDb == midDb)&&(identical(other.highDb, highDb) || other.highDb == highDb)&&(identical(other.preset, preset) || other.preset == preset)&&(identical(other.safety, safety) || other.safety == safety));
}


@override
int get hashCode => Object.hash(runtimeType,gainDb,muted,lowDb,midDb,highDb,preset,safety);

@override
String toString() {
  return 'MonitorSettings(gainDb: $gainDb, muted: $muted, lowDb: $lowDb, midDb: $midDb, highDb: $highDb, preset: $preset, safety: $safety)';
}


}

/// @nodoc
abstract mixin class $MonitorSettingsCopyWith<$Res>  {
  factory $MonitorSettingsCopyWith(MonitorSettings value, $Res Function(MonitorSettings) _then) = _$MonitorSettingsCopyWithImpl;
@useResult
$Res call({
 double gainDb, bool muted, double lowDb, double midDb, double highDb, BufferPreset preset, SafetyProfile safety
});




}
/// @nodoc
class _$MonitorSettingsCopyWithImpl<$Res>
    implements $MonitorSettingsCopyWith<$Res> {
  _$MonitorSettingsCopyWithImpl(this._self, this._then);

  final MonitorSettings _self;
  final $Res Function(MonitorSettings) _then;

/// Create a copy of MonitorSettings
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') @override $Res call({Object? gainDb = null,Object? muted = null,Object? lowDb = null,Object? midDb = null,Object? highDb = null,Object? preset = null,Object? safety = null,}) {
  return _then(_self.copyWith(
gainDb: null == gainDb ? _self.gainDb : gainDb // ignore: cast_nullable_to_non_nullable
as double,muted: null == muted ? _self.muted : muted // ignore: cast_nullable_to_non_nullable
as bool,lowDb: null == lowDb ? _self.lowDb : lowDb // ignore: cast_nullable_to_non_nullable
as double,midDb: null == midDb ? _self.midDb : midDb // ignore: cast_nullable_to_non_nullable
as double,highDb: null == highDb ? _self.highDb : highDb // ignore: cast_nullable_to_non_nullable
as double,preset: null == preset ? _self.preset : preset // ignore: cast_nullable_to_non_nullable
as BufferPreset,safety: null == safety ? _self.safety : safety // ignore: cast_nullable_to_non_nullable
as SafetyProfile,
  ));
}

}


/// Adds pattern-matching-related methods to [MonitorSettings].
extension MonitorSettingsPatterns on MonitorSettings {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>(TResult Function( _MonitorSettings value)?  $default,{required TResult orElse(),}){
final _that = this;
switch (_that) {
case _MonitorSettings() when $default != null:
return $default(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>(TResult Function( _MonitorSettings value)  $default,){
final _that = this;
switch (_that) {
case _MonitorSettings():
return $default(_that);case _:
  throw StateError('Unexpected subclass');

}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>(TResult? Function( _MonitorSettings value)?  $default,){
final _that = this;
switch (_that) {
case _MonitorSettings() when $default != null:
return $default(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>(TResult Function( double gainDb,  bool muted,  double lowDb,  double midDb,  double highDb,  BufferPreset preset,  SafetyProfile safety)?  $default,{required TResult orElse(),}) {final _that = this;
switch (_that) {
case _MonitorSettings() when $default != null:
return $default(_that.gainDb,_that.muted,_that.lowDb,_that.midDb,_that.highDb,_that.preset,_that.safety);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>(TResult Function( double gainDb,  bool muted,  double lowDb,  double midDb,  double highDb,  BufferPreset preset,  SafetyProfile safety)  $default,) {final _that = this;
switch (_that) {
case _MonitorSettings():
return $default(_that.gainDb,_that.muted,_that.lowDb,_that.midDb,_that.highDb,_that.preset,_that.safety);case _:
  throw StateError('Unexpected subclass');

}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>(TResult? Function( double gainDb,  bool muted,  double lowDb,  double midDb,  double highDb,  BufferPreset preset,  SafetyProfile safety)?  $default,) {final _that = this;
switch (_that) {
case _MonitorSettings() when $default != null:
return $default(_that.gainDb,_that.muted,_that.lowDb,_that.midDb,_that.highDb,_that.preset,_that.safety);case _:
  return null;

}
}

}

/// @nodoc


class _MonitorSettings extends MonitorSettings {
  const _MonitorSettings({this.gainDb = -6.0, this.muted = false, this.lowDb = 0.0, this.midDb = 0.0, this.highDb = 0.0, this.preset = BufferPreset.normal, this.safety = SafetyProfile.protected}): super._();
  

@override@JsonKey() final  double gainDb;
@override@JsonKey() final  bool muted;
@override@JsonKey() final  double lowDb;
@override@JsonKey() final  double midDb;
@override@JsonKey() final  double highDb;
@override@JsonKey() final  BufferPreset preset;
@override@JsonKey() final  SafetyProfile safety;

/// Create a copy of MonitorSettings
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
_$MonitorSettingsCopyWith<_MonitorSettings> get copyWith => __$MonitorSettingsCopyWithImpl<_MonitorSettings>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is _MonitorSettings&&(identical(other.gainDb, gainDb) || other.gainDb == gainDb)&&(identical(other.muted, muted) || other.muted == muted)&&(identical(other.lowDb, lowDb) || other.lowDb == lowDb)&&(identical(other.midDb, midDb) || other.midDb == midDb)&&(identical(other.highDb, highDb) || other.highDb == highDb)&&(identical(other.preset, preset) || other.preset == preset)&&(identical(other.safety, safety) || other.safety == safety));
}


@override
int get hashCode => Object.hash(runtimeType,gainDb,muted,lowDb,midDb,highDb,preset,safety);

@override
String toString() {
  return 'MonitorSettings(gainDb: $gainDb, muted: $muted, lowDb: $lowDb, midDb: $midDb, highDb: $highDb, preset: $preset, safety: $safety)';
}


}

/// @nodoc
abstract mixin class _$MonitorSettingsCopyWith<$Res> implements $MonitorSettingsCopyWith<$Res> {
  factory _$MonitorSettingsCopyWith(_MonitorSettings value, $Res Function(_MonitorSettings) _then) = __$MonitorSettingsCopyWithImpl;
@override @useResult
$Res call({
 double gainDb, bool muted, double lowDb, double midDb, double highDb, BufferPreset preset, SafetyProfile safety
});




}
/// @nodoc
class __$MonitorSettingsCopyWithImpl<$Res>
    implements _$MonitorSettingsCopyWith<$Res> {
  __$MonitorSettingsCopyWithImpl(this._self, this._then);

  final _MonitorSettings _self;
  final $Res Function(_MonitorSettings) _then;

/// Create a copy of MonitorSettings
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? gainDb = null,Object? muted = null,Object? lowDb = null,Object? midDb = null,Object? highDb = null,Object? preset = null,Object? safety = null,}) {
  return _then(_MonitorSettings(
gainDb: null == gainDb ? _self.gainDb : gainDb // ignore: cast_nullable_to_non_nullable
as double,muted: null == muted ? _self.muted : muted // ignore: cast_nullable_to_non_nullable
as bool,lowDb: null == lowDb ? _self.lowDb : lowDb // ignore: cast_nullable_to_non_nullable
as double,midDb: null == midDb ? _self.midDb : midDb // ignore: cast_nullable_to_non_nullable
as double,highDb: null == highDb ? _self.highDb : highDb // ignore: cast_nullable_to_non_nullable
as double,preset: null == preset ? _self.preset : preset // ignore: cast_nullable_to_non_nullable
as BufferPreset,safety: null == safety ? _self.safety : safety // ignore: cast_nullable_to_non_nullable
as SafetyProfile,
  ));
}


}

// dart format on
