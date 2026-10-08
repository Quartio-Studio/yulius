import { $f as f32, Ac as defineStd, Al as uniformOf, Dc as vec3, Ic as animatedTime, Ls as softDisc, Ml as p, Oc as vec4, Ra as shine, Ts as driveUnitDirection, Ws as add, bc as smoothstep, cr as rawSurfaceOf, dc as mul, ir as centredFrame, kl as resolveScalar, oc as local, ou as colorStopsPropConfig, qo as stops, sa as waves, va as domeNormal } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var WOBBLE = [
	{
		amount: .15,
		schedule: [{ waves: [{
			x: 3.2,
			t: .8
		}, {
			y: 2.8,
			t: .6
		}] }, { waves: [{
			x: 4.8,
			y: -3.6,
			t: -.4
		}] }]
	},
	{
		amount: .12,
		schedule: [{ waves: [{
			x: 5.6,
			t: -.5
		}, {
			y: 4.4,
			t: .7
		}] }]
	},
	{
		amount: .1,
		schedule: [{ waves: [{
			x: 7.2,
			y: 6.4,
			t: .3
		}] }, { waves: [{
			x: 2.4,
			t: -.9
		}] }]
	},
	{
		amount: .04,
		schedule: [{ waves: [{
			x: 8.8,
			t: .2
		}, {
			y: 7.6,
			t: -.8
		}] }]
	}
];
var FILL_DRIFT_A = [{ waves: [{
	x: 1,
	t: .4
}, {
	y: 1,
	t: .3
}] }, { waves: [{
	x: .7,
	y: .8,
	t: .2
}] }];
var FILL_DRIFT_B = [{ waves: [{
	x: 1.3,
	t: -.5
}, {
	y: 1.1,
	t: .6
}] }, { waves: [{
	x: .5,
	y: -.6,
	t: .3
}] }];
const componentDefinition = defineStd({
	name: "Blob",
	role: "generator",
	category: "Textures",
	description: "Organic animated blob with 3D lighting and gradients",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	extraFields: {
		normLx: {
			schema: f32,
			initial: 0
		},
		normLy: {
			schema: f32,
			initial: 0
		},
		normLz: {
			schema: f32,
			initial: 1
		}
	},
	boundingBoxDeclaration: {
		propBindings: {
			x: {
				prop: "center",
				as: "position-x"
			},
			y: {
				prop: "center",
				as: "position-y"
			}
		},
		computeBounds(props, cw, ch) {
			const size = props.size ?? .5;
			const cx = typeof props.center?.x === "number" ? props.center.x : .5;
			const cy = typeof props.center?.y === "number" ? props.center.y : .5;
			const diameter = size * 2 * ch;
			return {
				centerXPx: cx * cw,
				centerYPx: cy * ch,
				widthPx: diameter,
				heightPx: diameter,
				rotationDeg: 0
			};
		},
		writeBounds(bounds, currentProps, cw, ch) {
			const newSize = Math.max(0, bounds.widthPx / (2 * ch));
			return {
				center: {
					...currentProps.center ?? {
						x: .5,
						y: .5
					},
					x: bounds.centerXPx / cw,
					y: bounds.centerYPx / ch
				},
				size: newSize
			};
		},
		softnessBinding: {
			prop: "softness",
			toPx: (v, _, ch) => v * .3 * ch,
			fromPx: (px, _, ch) => Math.max(0, Math.min(1, px / (.3 * ch)))
		}
	},
	props: {
		origin: {
			default: "center",
			description: "Reference edge the center position is measured from (center default). Lets you pin the shape relative to a corner or the canvas centre.",
			ui: {
				type: "origin",
				label: "Origin",
				group: "Position"
			}
		},
		colorA: {
			default: "#ff6b35",
			transform: transformColor,
			description: "Primary color of the blob",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#e91e63",
			transform: transformColor,
			description: "Secondary color of the blob",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		size: {
			default: .5,
			description: "Size of the blob",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Size",
				group: "Effect"
			}
		},
		deformation: {
			default: .5,
			description: "How organic and blobby the shape is (0 = circle, 1 = very blobby)",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .1,
				label: "Deformation",
				group: "Effect"
			}
		},
		softness: {
			default: .5,
			description: "Softness of the blob edges (combines edge width and transition curve)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Softness",
				group: "Effect"
			}
		},
		highlightIntensity: {
			default: .5,
			description: "Intensity of specular highlight effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Highlight Intensity",
				group: "Highlight"
			}
		},
		highlightX: {
			default: .3,
			description: "Light direction X component",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .1,
				label: "Highlight X",
				group: "Highlight"
			}
		},
		highlightY: {
			default: -.3,
			description: "Light direction Y component",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .1,
				label: "Highlight Y",
				group: "Highlight"
			}
		},
		highlightZ: {
			default: .4,
			description: "Light direction Z component",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .1,
				label: "Highlight Z",
				group: "Highlight"
			}
		},
		highlightColor: {
			default: "#ffe11a",
			transform: transformColor,
			description: "Color of the specular highlight",
			ui: {
				type: "color",
				label: "Highlight Color",
				group: "Highlight"
			}
		},
		speed: {
			default: .5,
			description: "Animation speed",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		seed: {
			default: 1,
			description: "Adjusts the starting state, useful for variation",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Effect"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "The center point of the blob",
			ui: {
				type: "position",
				label: "Center Position",
				group: "Position",
				units: ["%", "px"]
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	},
	paint: (params) => {
		const { uniforms } = params;
		const scalar = (ref) => resolveScalar(ref, params);
		driveUnitDirection(params, {
			x: "highlightX",
			y: "highlightY",
			z: "highlightZ"
		}, {
			x: "normLx",
			y: "normLy",
			z: "normLz"
		});
		const t = local(animatedTime(params, "seed"), "blobTime");
		const { delta, dist } = centredFrame({ center: p("center") })(params, rawSurfaceOf(params));
		const axes = {
			x: delta.member("x"),
			y: delta.member("y"),
			t
		};
		const deform = scalar(p("deformation"));
		const radius = local(add(scalar(p("size")), WOBBLE.map(({ schedule, amount }) => mul(mul(waves(axes, schedule), amount), deform)).reduce((a, b) => add(a, b))), "blobRadius");
		const softness = local(scalar(p("softness")), "blobSoftness");
		const mask = local(softDisc({
			dist,
			radius,
			softness: {
				width: mul(softness, .3),
				curve: add(mul(softness, 2), .5)
			}
		}), "blobMask");
		const highlight = mul(mul(mul(shine({
			normal: domeNormal({
				delta,
				dist,
				radius
			}),
			light: vec3(uniforms.normLx, uniforms.normLy, uniforms.normLz),
			gloss: 32
		}), add(mul(smoothstep(0, radius, dist), .5), .5)), scalar(p("highlightIntensity"))), mask);
		const fillAxes = {
			x: mul(axes.x, 3),
			y: mul(axes.y, 3),
			t
		};
		const drift = (schedule) => add(mul(waves(fillAxes, schedule), .5), .5);
		const fill = stops(p("colorSpace"))(smoothstep(.1, .9, add(mul(drift(FILL_DRIFT_A), .6), mul(drift(FILL_DRIFT_B), .4))), params);
		const highlightColor = uniformOf(p("highlightColor"), params);
		const avgAlpha = mul(add(uniforms.colorA.member("a"), uniforms.colorB.member("a")), .5);
		return vec4(add(fill.member("rgb"), mul(highlightColor.member("rgb"), highlight)), mul(avgAlpha, mask));
	}
});
var Blob_default = componentDefinition;
export { componentDefinition as n, Blob_default as t };
