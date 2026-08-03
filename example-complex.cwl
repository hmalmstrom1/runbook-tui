cwlVersion: v1.2
class: Workflow

requirements:
  - class: InlineJavascriptRequirement
  - class: ScatterFeatureRequirement

inputs:
  names:
    type: string[]
    default: ["Alice", "Bob", "Carol"]
  shout:
    type: boolean
    default: true

steps:
  greet:
    run:
      class: ExpressionTool
      inputs:
        name:
          type: string
      outputs:
        greeting:
          type: string
      expression: |
        ${
          return {"greeting": "Hello, " + inputs.name + "!"};
        }
    in:
      name: names
    scatter: [name]
    out: [greeting]

  maybe_shout:
    run:
      class: ExpressionTool
      inputs:
        greetings:
          type: string[]
      outputs:
        shouted:
          type: string[]
      expression: |
        ${
          return {"shouted": inputs.greetings.map(function(g) { return g.toUpperCase(); })};
        }
    in:
      greetings: greet/greeting
    when: |
      ${
        return inputs.greetings.length > 0;
      }
    out: [shouted]

outputs:
  all_greetings:
    type: string[]
    outputSource: greet/greeting
  shouted:
    type: string[]?
    outputSource: maybe_shout/shouted
