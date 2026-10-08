import { Ac as defineStd, Ec as vec2, Hd as call, Kc as flowDirectionColor, Ws as add, id as toHalfFloat, nt as hostGridProgram, rt as hostStep, tt as hostFieldTexture } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
var GRID_SIZE = 128;
var num = (v, f) => typeof v === "number" ? v : f;
const componentDefinition = defineStd({
	name: "ChromaFlow",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "Interactive liquid flow effect that follows your cursor",
	usesPointer: true,
	props: {
		baseColor: {
			default: "#0066ff",
			transform: transformColor,
			description: "Base liquid color",
			ui: {
				type: "color",
				label: "Base Color",
				group: "Colors"
			}
		},
		upColor: {
			default: "#00ff00",
			transform: transformColor,
			description: "Color for upward movement",
			ui: {
				type: "color",
				label: "Up Color",
				group: "Colors"
			}
		},
		downColor: {
			default: "#ff0000",
			transform: transformColor,
			description: "Color for downward movement",
			ui: {
				type: "color",
				label: "Down Color",
				group: "Colors"
			}
		},
		leftColor: {
			default: "#0000ff",
			transform: transformColor,
			description: "Color for leftward movement",
			ui: {
				type: "color",
				label: "Left Color",
				group: "Colors"
			}
		},
		rightColor: {
			default: "#ffff00",
			transform: transformColor,
			description: "Color for rightward movement",
			ui: {
				type: "color",
				label: "Right Color",
				group: "Colors"
			}
		},
		intensity: {
			default: 1,
			description: "Strength of the liquid effect",
			ui: {
				type: "range",
				min: .5,
				max: 1.5,
				step: .1,
				label: "Intensity",
				group: "Effect"
			}
		},
		radius: {
			default: 3,
			description: "Radius of the liquid effect",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .1,
				label: "Radius",
				group: "Effect"
			}
		},
		momentum: {
			default: 30,
			description: "How much momentum colors retain in their flow direction",
			ui: {
				type: "range",
				min: 10,
				max: 60,
				step: 1,
				label: "Momentum",
				group: "Effect"
			}
		}
	},
	gpu: { fragment: (params) => {
		const { uniforms, ctx, uvContext, onBeforeRender, dimensions, getCpuValue } = params;
		const fieldData = new Float32Array(GRID_SIZE * GRID_SIZE * 4);
		const tempFieldData = new Float32Array(GRID_SIZE * GRID_SIZE * 4);
		const field = hostFieldTexture(params, {
			size: GRID_SIZE,
			label: "chromaflow-field"
		});
		const fieldKit = field.texture;
		let mouseVelX = 0, mouseVelY = 0;
		let prevX = .5, prevY = .5;
		let fieldMax = 0;
		let aspect = 1;
		let injecting = false;
		let newMax = 0;
		let intensity = 1;
		let radius = .15;
		onBeforeRender(hostGridProgram({
			clampDt: .016,
			steps: [
				hostStep("cursorVelocity", (f) => {
					aspect = dimensions.width / Math.max(1, dimensions.height);
					const velX = f.dt > 0 ? (f.pointer.x - prevX) / f.dt : 0;
					const velY = f.dt > 0 ? (f.pointer.y - prevY) / f.dt : 0;
					mouseVelX = mouseVelX * .85 + velX * .15;
					mouseVelY = mouseVelY * .85 + velY * .15;
					prevX = f.pointer.x;
					prevY = f.pointer.y;
					injecting = Math.abs(velX) + Math.abs(velY) > .01;
				}),
				hostStep("idleSkip", () => !injecting && fieldMax < 1e-4 ? "skip" : void 0),
				hostStep("advect+decay", (f) => {
					intensity = num(getCpuValue("intensity"), 1);
					radius = num(getCpuValue("radius"), 3) * .05;
					const momentum = num(getCpuValue("momentum"), 30);
					const flowFadeRate = 1 - f.dt / Math.max(.1, 1);
					const liquidFadeRate = 1 - f.dt;
					const flowSpeed = momentum * 50 * f.dt;
					newMax = 0;
					for (let i = 0; i < GRID_SIZE; i++) for (let j = 0; j < GRID_SIZE; j++) {
						const idx = (i * GRID_SIZE + j) * 4;
						const fx0 = fieldData[idx];
						const fy0 = fieldData[idx + 1];
						tempFieldData[idx] = fx0 * flowFadeRate;
						tempFieldData[idx + 1] = fy0 * flowFadeRate;
						tempFieldData[idx + 2] = fieldData[idx + 2] * liquidFadeRate;
						if (Math.abs(fx0) > .001 || Math.abs(fy0) > .001) {
							const advectX = j - fx0 * flowSpeed;
							const advectY = i - fy0 * flowSpeed;
							const x0 = Math.floor(advectX);
							const y0 = Math.floor(advectY);
							const x1 = x0 + 1;
							const y1 = y0 + 1;
							if (x0 >= 0 && y0 >= 0 && x1 < GRID_SIZE && y1 < GRID_SIZE) {
								const fx = advectX - x0;
								const fy = advectY - y0;
								const idx00 = (y0 * GRID_SIZE + x0) * 4;
								const idx01 = (y0 * GRID_SIZE + x1) * 4;
								const idx10 = (y1 * GRID_SIZE + x0) * 4;
								const idx11 = (y1 * GRID_SIZE + x1) * 4;
								const sampledLiquid = fieldData[idx00 + 2] * (1 - fx) * (1 - fy) + fieldData[idx01 + 2] * fx * (1 - fy) + fieldData[idx10 + 2] * (1 - fx) * fy + fieldData[idx11 + 2] * fx * fy;
								tempFieldData[idx + 2] = sampledLiquid * liquidFadeRate;
							}
						}
						const m = Math.max(Math.abs(tempFieldData[idx]), Math.abs(tempFieldData[idx + 1]), tempFieldData[idx + 2]);
						if (m > newMax) newMax = m;
					}
				}),
				hostStep("inject(brush)", (f) => {
					if (!injecting) return;
					const speed = Math.sqrt(mouseVelX * mouseVelX + mouseVelY * mouseVelY);
					const speedScale = Math.min(speed * speed * 20, 1);
					const effectiveRadius = radius * speedScale;
					const maxDistSq = effectiveRadius * 2 * (effectiveRadius * 2);
					const radSq = effectiveRadius * effectiveRadius;
					const addScale = intensity * 100 * f.dt * .01;
					const speedMultiplier = Math.min(speed * 10, 1);
					for (let i = 0; i < GRID_SIZE; i++) for (let j = 0; j < GRID_SIZE; j++) {
						const cellX = (j + .5) / GRID_SIZE;
						const cellY = (i + .5) / GRID_SIZE;
						const dx = aspect >= 1 ? (cellX - f.pointer.x) * aspect : cellX - f.pointer.x;
						const dy = aspect >= 1 ? cellY - f.pointer.y : (cellY - f.pointer.y) / aspect;
						const distSq = dx * dx + dy * dy;
						if (distSq < maxDistSq) {
							const idx = (i * GRID_SIZE + j) * 4;
							const influence = Math.exp(-distSq / radSq);
							tempFieldData[idx] += mouseVelX * influence * addScale;
							tempFieldData[idx + 1] += mouseVelY * influence * addScale;
							tempFieldData[idx + 2] += influence * addScale * speedMultiplier;
							tempFieldData[idx] = Math.max(-1, Math.min(1, tempFieldData[idx]));
							tempFieldData[idx + 1] = Math.max(-1, Math.min(1, tempFieldData[idx + 1]));
							tempFieldData[idx + 2] = Math.max(0, Math.min(1, tempFieldData[idx + 2]));
						}
					}
				}),
				hostStep("publish(halfFloat)", () => {
					fieldMax = injecting ? 1 : newMax;
					fieldData.set(tempFieldData);
					for (let k = 0; k < field.texData.length; k++) field.texData[k] = toHalfFloat(fieldData[k]);
					field.upload();
				})
			]
		}));
		const uv = uvContext ?? ctx.uv;
		const px = 1 / GRID_SIZE;
		const clamp = "linearClamp";
		const tap = (dx, dy) => fieldKit.sample(add(uv, vec2(dx, dy)), clamp);
		return call(flowDirectionColor, "flowDirectionColor", [
			tap(0, 0),
			tap(px, 0),
			tap(0, px),
			tap(-px, 0),
			tap(0, -px),
			uniforms.baseColor,
			uniforms.upColor,
			uniforms.downColor,
			uniforms.leftColor,
			uniforms.rightColor
		]);
	} }
});
var ChromaFlow_default = componentDefinition;
export { componentDefinition as n, ChromaFlow_default as t };
