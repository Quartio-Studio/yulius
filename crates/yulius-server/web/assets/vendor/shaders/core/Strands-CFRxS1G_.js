import { Ac as defineStd, Ji as overRgb, Ml as p, Ni as backToP3, Qi as ribbons, na as tonePow, ou as colorStopsPropConfig } from "./std-ClftfWpF.js";
import { c as transformPosition, i as transformBoolean, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Strands",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Flowing ribbons of light with a multi-color gradient",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	extraAnimatedTimes: { color: "colorSpeed" },
	props: {
		speed: {
			default: .5,
			description: "Overall animation speed",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		amplitude: {
			default: 2,
			description: "How far the strands wave away from their resting line",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 5,
				step: .1,
				label: "Amplitude",
				group: "Effect"
			}
		},
		frequency: {
			default: .3,
			description: "How many waves run along each strand",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 5,
				step: .1,
				label: "Frequency",
				group: "Effect"
			}
		},
		lineCount: {
			default: 8,
			description: "Number of strands",
			ui: {
				type: "range",
				min: 1,
				max: 16,
				step: 1,
				label: "Strand Count",
				group: "Effect"
			}
		},
		lineWidth: {
			default: .05,
			description: "Thickness of each strand’s solid core",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .05,
				label: "Strand Width",
				group: "Effect"
			}
		},
		softness: {
			default: .05,
			description: "Softness of each strand’s edge (0 is crisp, higher is feathered)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .05,
				label: "Strand Softness",
				group: "Effect"
			}
		},
		spread: {
			default: .2,
			description: "How far the strands fan out across the field",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .05,
				label: "Spread",
				group: "Effect"
			}
		},
		pinEdges: {
			default: false,
			transform: transformBoolean,
			description: "Pin the strands so they converge to the start and end points",
			ui: {
				type: "checkbox",
				label: "Pin Edges",
				group: "Effect"
			}
		},
		stops: {
			...colorStopsPropConfig(),
			default: [
				{
					color: "#00e5ff",
					position: 0
				},
				{
					color: "#5b8cff",
					position: .34
				},
				{
					color: "#b06bff",
					position: .67
				},
				{
					color: "#ff5fa2",
					position: 1
				}
			]
		},
		colorSpace: {
			default: "oklab",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space used to blend the gradient",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		},
		colorScale: {
			default: 1,
			description: "How many times the gradient repeats along the strands",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 6,
				step: 1,
				label: "Color Scale",
				group: "Colors"
			}
		},
		colorVariance: {
			default: 1,
			description: "At 0 every strand shares the same color; at 1 the strands spread across the full gradient",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .05,
				label: "Color Variance",
				group: "Colors"
			}
		},
		colorSpeed: {
			default: 1,
			description: "Speed at which the colors flow along the strands (loops)",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .05,
				label: "Color Speed",
				group: "Colors"
			}
		},
		start: {
			default: {
				x: 0,
				y: .5
			},
			transform: transformPosition,
			description: "Starting point of the strands",
			ui: {
				type: "position",
				label: "Start",
				group: "Position"
			}
		},
		end: {
			default: {
				x: 1,
				y: .5
			},
			transform: transformPosition,
			description: "Ending point of the strands",
			ui: {
				type: "position",
				label: "End",
				group: "Position"
			}
		}
	},
	paint: overRgb(ribbons({
		from: p("start"),
		to: p("end"),
		count: p("lineCount"),
		width: p("lineWidth"),
		amplitude: p("amplitude"),
		frequency: p("frequency"),
		softness: p("softness"),
		spread: p("spread"),
		pinEdges: p("pinEdges"),
		colorScale: p("colorScale"),
		colorVariance: p("colorVariance")
	}), backToP3({ space: p("colorSpace") }), tonePow())
});
var Strands_default = componentDefinition;
export { componentDefinition as n, Strands_default as t };
