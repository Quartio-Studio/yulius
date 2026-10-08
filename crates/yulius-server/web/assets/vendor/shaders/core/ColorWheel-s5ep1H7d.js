import { Ac as defineStd, Ml as p, wo as colorWheel } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, r as transformAngle, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "ColorWheel",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "A directional gradient that smoothly cycles through rainbow colors or a custom set of three colors",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		mode: {
			default: "rainbow",
			compileTime: true,
			description: "Rainbow cycles through the full spectrum; Custom loops through your three chosen colors",
			ui: {
				type: "select",
				options: [{
					label: "Rainbow",
					value: "rainbow"
				}, {
					label: "Custom",
					value: "custom"
				}],
				label: "Mode",
				group: "Colors"
			}
		},
		colorA: {
			default: "#ff0000",
			transform: transformColor,
			description: "First color in the cycle",
			ui: {
				type: "color",
				label: "Color 1",
				group: "Colors",
				condition: { mode: "custom" }
			}
		},
		colorB: {
			default: "#00ff88",
			transform: transformColor,
			description: "Second color in the cycle",
			ui: {
				type: "color",
				label: "Color 2",
				group: "Colors",
				condition: { mode: "custom" }
			}
		},
		colorC: {
			default: "#0066ff",
			transform: transformColor,
			description: "Third color in the cycle",
			ui: {
				type: "color",
				label: "Color 3",
				group: "Colors",
				condition: { mode: "custom" }
			}
		},
		scale: {
			default: 1,
			description: "Number of color cycles across the viewport",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 10,
				step: .1,
				label: "Scale",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			transform: transformAngle,
			description: "Direction the gradient flows",
			ui: {
				type: ["range", "map"],
				min: -180,
				max: 180,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		speed: {
			default: .05,
			description: "Speed at which the gradient cycles",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .01,
				label: "Speed",
				group: "Animation"
			}
		},
		colorSpace: {
			default: "oklch",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for blending between custom colors",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors",
				condition: { mode: "custom" }
			}
		}
	},
	paint: colorWheel({
		mode: p("mode"),
		direction: p("angle"),
		scale: p("scale"),
		palette: {
			a: p("colorA"),
			b: p("colorB"),
			c: p("colorC")
		},
		space: p("colorSpace")
	})
});
var ColorWheel_default = componentDefinition;
export { componentDefinition as n, ColorWheel_default as t };
