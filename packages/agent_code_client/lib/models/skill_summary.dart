/// A user-invocable skill, as listed by the agent for the composer's slash
/// picker.
///
/// Carries what the picker needs to render a row and nothing more. The skill
/// body is prompt text the agent expands server-side; it is deliberately not
/// part of this model.
class SkillSummary {
  final String name;
  final String? description;

  /// Autocomplete hint for the skill's arguments, e.g. `commit message`.
  final String? argumentHint;

  const SkillSummary({
    required this.name,
    this.description,
    this.argumentHint,
  });

  factory SkillSummary.fromJson(Map<String, dynamic> json) => SkillSummary(
        name: json['name'] as String? ?? '',
        description: json['description'] as String?,
        argumentHint: json['argument_hint'] as String?,
      );

  Map<String, dynamic> toJson() => {
        'name': name,
        if (description != null) 'description': description,
        if (argumentHint != null) 'argument_hint': argumentHint,
      };
}
